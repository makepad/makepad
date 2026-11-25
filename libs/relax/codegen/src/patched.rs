//! A live session's split root build (`split_patched_path`). Its first inlining round guarded each
//! copy of a patchable body (`opt::inline_site`): a compare of the build's copy version, a block
//! that reads the copy's flags, a slow call of its slot that rejoins the copy's continuation. Those
//! slow calls cost the optimiser more than the reads do: the values they take stay in memory and
//! their results merge back into the fast path. So the body is split before it is optimised: the
//! fast half runs every copy unguarded, and an exit in each stretch that holds one (after a call
//! or a loop's back edge, where a frame may have waited for a patch), before its first copy where
//! the patched path needs the least from it, reads the copy version once. While it holds the value the build read, no flag of a copy the code holds was
//! set; once it moved, the exit leaves for the patched path, a copy of the rest of the fn whose
//! copies keep their guards, and never comes back. The fast half then holds no guard and no slow
//! call, and the optimiser sees the code a run without patches builds.
//!
//! A patch reaches running frames as the guards' did: new calls run new code (reload rebuilds the
//! fns that inlined the body), a frame that returned from a call or crossed a back edge leaves at
//! its next exit, a stretch with no wait point may finish its old copies.

use relax_backend::passes::map_inst;
use relax_backend::regalloc::{for_defs, for_uses, term_uses, term_vregs_mut};
use relax_backend::rir::*;

/// What `split_patched_path` made (the inline trace).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SplitStats {
    /// exits the first copy site of a stretch needs (each where it carries the least)
    pub site_exits: u32,
    /// exits a call the second inlining round may take in needs (provisional: deleted when the
    /// round does not take it in, `prune_exits`)
    pub call_exits: u32,
    /// copy sites an earlier exit of their stretch covers: their guards are gone
    pub covered: u32,
    /// the patched path: blocks and instructions
    pub blocks: u32,
    pub insts: u32,
    /// slots the patched path holds a copy of (filled at each exit), and slots both halves share
    pub cloned: u32,
    pub shared: u32,
    /// the exits' fills (an exit and a slot it fills) and the bytes they fill
    pub filled: u32,
    pub fill_bytes: u32,
    /// registers the exits move into the patched path
    pub moved: u32,
}

/// What a taken exit must pass to the private continuation. Slot and register
/// numbers here are those of the split body, before either half is optimised.
#[derive(Debug, PartialEq)]
pub enum RecordValue {
    Register { src: VReg, dst: VReg, class: Cls, result_buffer: bool },
    Fill { src: u32, dst: u32, offset: i64, kind: FillKind },
    SharedBase(u32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FillKind {
    Cell(Mem),
    Vector,
    Bytes(u32),
}

impl RecordValue {
    fn size_align(&self) -> Result<(u32, u32), &'static str> {
        let n = match *self {
            RecordValue::Register { class: Cls::I | Cls::F64, .. } | RecordValue::SharedBase(_) => 8,
            RecordValue::Register { class: Cls::F32, .. } => 4,
            RecordValue::Register { class: Cls::V128, .. } | RecordValue::Fill { kind: FillKind::Vector, .. } => 16,
            RecordValue::Fill { kind: FillKind::Cell(m @ (Mem::Int(1 | 2 | 4 | 8, _) | Mem::F32 | Mem::F64)), .. } => relax_backend::passes::mem_size(m) as u32,
            RecordValue::Fill { kind: FillKind::Bytes(n), .. } if n != 0 => return Ok((n, 1)),
            _ => return Err("unsupported record field"),
        };
        Ok((n, n))
    }
}

pub struct ExitLayout {
    pub id: u32,
    pub block: u32,
    pub target: u32,
    /// (byte offset in the record, value)
    pub fields: Vec<(u32, RecordValue)>,
    /// Clone-relative addresses built by the parallel copy belong in P's entry leaf.
    pub remakes: Vec<Inst>,
}

pub struct SplitLayout {
    pub region: std::ops::Range<usize>,
    pub exits: Vec<ExitLayout>,
    /// Size of the single 16-byte aligned slot to allocate upon extraction.
    pub record_size: u32,
    /// The split's maps, also used to move the source locals into P's frame.
    pub reg_map: Vec<u32>,
    pub slot_map: Vec<u32>,
    shared: Vec<u32>,
}

/// Each entry needs the shared bases mentioned by any successor, pads included.
/// Use one bit per shared slot, irrespective of the number of host-only slots.
fn shared_bases(f: &Func, region: std::ops::Range<usize>, shared: &[u32]) -> Vec<u64> {
    if shared.is_empty() { return Vec::new(); }
    let mut index = vec![NONE; f.slots.len()];
    for (k, &s) in shared.iter().enumerate() { index[s as usize] = k as u32; }
    let words = shared.len().div_ceil(64);
    let blocks: Vec<_> = region.collect();
    let mut at = vec![NONE; f.blocks.len()];
    let mut gen = vec![0; blocks.len() * words];
    for (k, &b) in blocks.iter().enumerate() {
        at[b] = k as u32;
        for i in &f.blocks[b].insts {
            if let Inst::SlotAddr(_, s) = *i {
                let bit = index[s as usize];
                if bit != NONE { gen[k * words + bit as usize / 64] |= 1 << (bit % 64); }
            }
        }
    }
    liveness_in(f, &blocks, &at, words, &gen, &vec![0; gen.len()])
}

#[derive(Default)]
struct ExitFills {
    values: Vec<RecordValue>,
    /// Parallel moves and address remakes follow these instructions in the exit.
    insts: usize,
}

fn capture_layout(f: &Func, region: std::ops::Range<usize>, reg_map: Vec<u32>, slot_map: Vec<u32>, shared: &[u32],
                  t: &Taint, fills: Vec<ExitFills>) -> Result<SplitLayout, &'static str> {
    let bases = shared_bases(f, region.clone(), shared);
    let words = shared.len().div_ceil(64);
    let mut exits = Vec::new();
    let mut record_size = 16;
    for (id, ExitFills { mut values, insts }) in fills.into_iter().enumerate() {
        let block = region.end + id;
        let b = &f.blocks[block];
        let Term::Jump(target) = b.term else { continue }; // an unreachable exit
        if !region.contains(&(target as usize)) { return Err("invalid exit target"); }
        let mut remakes = Vec::new();
        for i in &b.insts[insts..] {
            match *i {
                Inst::Mov(dst, src) => {
                    let slot = t.slot[src.0 as usize];
                    if slot == MIXED || slot != NONE && (t.other[src.0 as usize] || slot_map[slot as usize] != NONE) {
                        return Err("ambiguous pointer provenance");
                    }
                    values.push(RecordValue::Register { src, dst, class: f.vregs[src.0 as usize], result_buffer: f.sret == Some(src) });
                }
                Inst::SlotAddr(..) | Inst::IBinI(..) => remakes.push(i.clone()),
                _ => return Err("unsupported exit move"),
            }
        }
        let row = (target as usize - region.start) * words;
        for (k, &s) in shared.iter().enumerate() {
            if bases[row + k / 64] >> (k % 64) & 1 != 0 { values.push(RecordValue::SharedBase(s)); }
        }
        let mut fields = Vec::with_capacity(values.len());
        let mut end = 0u32;
        for value in values {
            let (n, align) = value.size_align()?;
            if let RecordValue::Fill { src, dst, offset, .. } = value {
                if offset < 0 || offset + i64::from(n) > i64::from(f.slots[src as usize].size.min(f.slots[dst as usize].size)) {
                    return Err("fill outside slot");
                }
            }
            let off = end.checked_next_multiple_of(align).ok_or("record offset overflow")?;
            end = off.checked_add(n).ok_or("record offset overflow")?;
            fields.push((off, value));
        }
        record_size = record_size.max(end.checked_next_multiple_of(16).filter(|n| *n <= i32::MAX as u32).ok_or("record offset overflow")?);
        exits.push(ExitLayout { id: id as u32, block: block as u32, target, fields, remakes });
    }
    Ok(SplitLayout { region, exits, record_size, reg_map, slot_map, shared: shared.to_vec() })
}

/// The body kind comes from the build job: a sandbox twin and its base can have
/// the same RIR name. The native producer must know which one it was given.
#[derive(Clone, Copy)]
pub enum OutlineBody { Native, SandboxTwin, Web, Interpreter, WholeBody }

pub struct OutlinedPart {
    pub part: Func,
    exits: Vec<ExitLayout>,
    region: std::ops::Range<usize>,
    record_size: u32,
    // Original normal and cleanup edges, and every unwind-resume, in P's numbering.
    unwind: Vec<(u32, Term)>,
    bases: Vec<VReg>,
    // Shared slots in P's current numbering. After verification compaction removes them all.
    shared_slots: Vec<u32>,
}

fn record_mem(c: Cls) -> Option<Mem> {
    match c {
        Cls::I => Some(Mem::Int(8, false)),
        Cls::F32 => Some(Mem::F32),
        Cls::F64 => Some(Mem::F64),
        Cls::V128 => None,
        Cls::V256 => unreachable!("unsupported record field"),
    }
}

fn load_field(m: Option<Mem>, v: VReg, base: VReg, off: i32) -> Inst {
    match m { Some(m) => Inst::Load(m, v, base, off), None => Inst::VLoad(v, base, off) }
}

fn store_field(m: Option<Mem>, base: VReg, off: i32, v: VReg) -> Inst {
    match m { Some(m) => Inst::Store(m, base, off, v), None => Inst::VStore(base, off, v) }
}

fn offset_addr(f: &mut Func, out: &mut Vec<Inst>, base: VReg, off: i64) -> VReg {
    if off == 0 { return base; }
    let d = f.vreg(Cls::I);
    out.push(Inst::IBinI(IOp::Add, U64, d, base, off));
    d
}

/// Generate both ends of a transfer together. The only address in a fill
/// crossing frames is the record itself; P restores the bytes to its own slot.
fn record_exit(f: &mut Func, p: &mut Func, e: &ExitLayout, slot: u32, bases: &[VReg], target: u32) -> (Block, Block) {
    let pos = f.blocks[e.block as usize].term_pos;
    let record = f.vreg(Cls::I);
    let arg = p.params[0];
    let mut pack = vec![Inst::SlotAddr(record, slot)];
    let mut unpack = Vec::new();
    for &(off, ref field) in &e.fields {
        let off = off as i32;
        match *field {
            RecordValue::Register { src, dst, class, .. } => {
                pack.push(store_field(record_mem(class), record, off, src));
                unpack.push(load_field(record_mem(class), dst, arg, off));
            }
            RecordValue::SharedBase(s) => {
                let a = f.vreg(Cls::I);
                pack.push(Inst::SlotAddr(a, s));
                pack.push(Inst::Store(Mem::Int(8, false), record, off, a));
                unpack.push(Inst::Load(Mem::Int(8, false), bases[s as usize], arg, off));
            }
            RecordValue::Fill { src, dst, offset, kind } => {
                let (a, b) = (f.vreg(Cls::I), p.vreg(Cls::I));
                pack.push(Inst::SlotAddr(a, src));
                unpack.push(Inst::SlotAddr(b, dst));
                match kind {
                    FillKind::Bytes(n) => {
                        let from = offset_addr(f, &mut pack, a, offset);
                        let to = offset_addr(f, &mut pack, record, off.into());
                        pack.push(Inst::Copy(to, from, n));
                        let from = offset_addr(p, &mut unpack, arg, off.into());
                        let to = offset_addr(p, &mut unpack, b, offset);
                        unpack.push(Inst::Copy(to, from, n));
                    }
                    _ => {
                        let m = match kind { FillKind::Cell(m) => Some(m), _ => None };
                        let c = match m { None => Cls::V128, Some(Mem::F32) => Cls::F32, Some(Mem::F64) => Cls::F64, _ => Cls::I };
                        let (v, w) = (f.vreg(c), p.vreg(c));
                        pack.extend([load_field(m, v, a, offset as i32), store_field(m, record, off, v)]);
                        unpack.extend([load_field(m, w, arg, off), store_field(m, b, offset as i32, w)]);
                    }
                }
            }
        }
    }
    unpack.extend(e.remakes.iter().cloned());
    let results: Vec<_> = p.rets.iter().map(|&c| f.vreg(c)).collect();
    pack.push(Inst::Call(Callee::Part(Some(e.id)), vec![record], results.clone()));
    let block = |insts: Vec<Inst>, term| Block { pos: vec![pos; insts.len()], insts, term, term_pos: pos, count: f.blocks[e.block as usize].count };
    (block(pack, Term::Ret(results)), block(unpack, Term::Jump(target)))
}

// The leaves are in permanent-ID order, including holes left by dead exits.
// The root is b0 even for a single leaf.
fn dispatch(p: &mut Func, b: u32, leaves: &[(u32, u32)]) {
    if let [(_, leaf)] = leaves {
        p.blocks[b as usize].term = Term::Jump(*leaf);
        return;
    }
    let (left, right) = leaves.split_at(leaves.len() / 2);
    let (l, r) = (p.block(), p.block());
    let cmp = p.vreg(Cls::I);
    p.blocks[b as usize].insts.push(Inst::ICmpI(Cond::Lt, false, cmp, p.params[1], right[0].0.into()));
    p.blocks[b as usize].pos.push(0);
    p.blocks[b as usize].term = Term::Branch(cmp, l, r);
    dispatch(p, l, left);
    dispatch(p, r, right);
}

// Read the actual parallel copy independently of the capture. A missing field
// must refuse before either function starts relying on the shortened layout.
fn check_transfers(f: &Func, e: &ExitLayout) -> Result<(), &'static str> {
    let insts = &f.blocks[e.block as usize].insts;
    let end = insts.iter().rposition(|i| matches!(i, Inst::Store(..) | Inst::VStore(..) | Inst::Copy(..))).map_or(0, |k| k + 1);
    let mut fields = e.fields.iter().map(|x| &x.1).filter(|x| !matches!(x, RecordValue::SharedBase(_)));
    let mut addrs = std::collections::HashMap::new();
    let mut loaded = std::collections::HashMap::new();
    let bad = "incomplete exit transfer";
    for i in &insts[..end] {
        let value = match *i {
            Inst::SlotAddr(a, s) => { addrs.insert(a, (s, 0i64)); continue; }
            Inst::IBinI(IOp::Add, U64, d, a, k) => {
                let &(s, off) = addrs.get(&a).ok_or(bad)?;
                addrs.insert(d, (s, off.checked_add(k).ok_or(bad)?));
                continue;
            }
            Inst::Load(_, v, a, k) | Inst::VLoad(v, a, k) => {
                let &(s, off) = addrs.get(&a).ok_or(bad)?;
                let kind = match *i { Inst::Load(m, ..) => FillKind::Cell(m), _ => FillKind::Vector };
                loaded.insert(v, (s, off + i64::from(k), kind));
                continue;
            }
            Inst::Store(_, a, k, v) | Inst::VStore(a, k, v) => {
                let &(src, offset, kind) = loaded.get(&v).ok_or(bad)?;
                let &(dst, off) = addrs.get(&a).ok_or(bad)?;
                let stored = match *i { Inst::Store(m, ..) => FillKind::Cell(m), _ => FillKind::Vector };
                if off + i64::from(k) != offset || kind != stored { return Err(bad); }
                RecordValue::Fill { src, dst, offset, kind }
            }
            Inst::Copy(d, s, n) => {
                let &(src, offset) = addrs.get(&s).ok_or(bad)?;
                let &(dst, off) = addrs.get(&d).ok_or(bad)?;
                if off != offset { return Err(bad); }
                RecordValue::Fill { src, dst, offset, kind: FillKind::Bytes(n) }
            }
            _ => return Err(bad),
        };
        if fields.next() != Some(&value) { return Err(bad); }
    }
    let mut remakes = e.remakes.iter();
    for i in &insts[end..] {
        match *i {
            Inst::Mov(dst, src) => {
                let value = RecordValue::Register { src, dst, class: f.vregs[src.0 as usize], result_buffer: f.sret == Some(src) };
                if fields.next() != Some(&value) { return Err(bad); }
            }
            _ if remakes.next() == Some(i) => {}
            _ => return Err(bad),
        }
    }
    if fields.next().is_some() || remakes.next().is_some() { return Err(bad); }
    Ok(())
}

/// Replace the split in place. The region's instruction vectors move to P.
/// On a verifier refusal the moves and the edge renumbering are reversible;
/// the only old blocks held aside are the small parallel copies at the exits.
pub fn extract_patched_part(f: &mut Func, layout: SplitLayout, body: OutlineBody) -> Result<OutlinedPart, &'static str> {
    match body {
        OutlineBody::SandboxTwin => return Err("sandbox twin"),
        OutlineBody::Web => return Err("web target"),
        OutlineBody::Interpreter => return Err("interpreter body"),
        OutlineBody::WholeBody => return Err("whole-body outline"),
        OutlineBody::Native => {}
    }
    if f.naked { return Err("naked function"); }
    if f.cabi.is_some() { return Err("C ABI host"); }
    if f.keep_frames { return Err("frame-sensitive function"); }
    let region = layout.region.clone();
    if region.start == 0 || region.is_empty() || region.end > f.blocks.len() || layout.exits.is_empty() { return Err("invalid patched region"); }
    if layout.record_size == 0 || layout.record_size % 16 != 0 || layout.record_size > i32::MAX as u32 { return Err("invalid record size"); }
    let mut p = Func::new(f.name.clone(), f.file);
    p.vregs = f.vregs.clone();
    p.slots = f.slots.clone();
    for s in &mut p.slots { s.born = false; }
    p.rets = f.rets.clone();
    let (arg, id) = (p.vreg(Cls::I), p.vreg(Cls::I));
    p.params.extend([arg, id]);
    p.baseline = true;
    p.inl = f.inl.clone();
    p.inl_slots = f.inl_slots.clone();
    p.inl_waits = f.inl_waits.clone();
    p.inl_digest = f.inl_digest.clone();
    p.guard_calls = f.guard_calls.clone();
    p.sret_callees = f.sret_callees.clone();
    p.dbg = f.dbg.clone();
    p.dbg_map_vregs(&|v| layout.reg_map.get(v.0 as usize).copied().filter(|&n| n != NONE).map(VReg));
    let mut bases = vec![DBG_NONE; f.slots.len()];
    let shared = &layout.shared;
    for &s in shared { bases[s as usize] = p.vreg(Cls::I); }
    let target = |t: u32| (t as usize - region.start + 1) as u32;
    let mut unwind = Vec::new();
    let mut last = None;
    let mut old_exit = vec![false; f.blocks.len() - region.end];
    for e in &layout.exits {
        if last.is_some_and(|id| id >= e.id) || e.block as usize != region.end + e.id as usize
            || !region.contains(&(e.target as usize)) || !f.blocks.get(e.block as usize).is_some_and(|b| b.term == Term::Jump(e.target)) {
            return Err("invalid exit target or ID");
        }
        check_transfers(f, e)?;
        old_exit[e.id as usize] = true;
        last = Some(e.id);
        let mut end = 0;
        for &(off, ref field) in &e.fields {
            let (n, align) = field.size_align()?;
            if off < end || off % align != 0 { return Err("invalid record field alignment"); }
            end = off.checked_add(n).filter(|&end| end <= layout.record_size).ok_or("record field out of bounds")?;
            match *field {
                RecordValue::Register { src, dst, class, result_buffer } => {
                    if [src, dst].iter().any(|v| f.vregs.get(v.0 as usize) != Some(&class)) || result_buffer != (f.sret == Some(src)) {
                        return Err("invalid register field");
                    }
                }
                RecordValue::SharedBase(s) => {
                    if bases.get(s as usize).is_none_or(|v| *v == DBG_NONE) { return Err("invalid shared base"); }
                }
                RecordValue::Fill { src, dst, offset, .. } => {
                    if layout.slot_map.get(src as usize) != Some(&dst) || offset < 0 || offset > i32::MAX as i64 ||
                        [src, dst].iter().any(|&s| f.slots.get(s as usize).is_none_or(|s| offset + i64::from(n) > i64::from(s.size))) {
                        return Err("fill outside slot");
                    }
                }
            }
        }
    }
    for (bi, b) in f.blocks.iter().enumerate() {
        let inside = region.contains(&bi);
        if b.insts.iter().any(|i| matches!(i, Inst::Call(Callee::Part(_), ..))) { return Err("nested part"); }
        if inside && b.insts.iter().any(|i| matches!(i, Inst::Call(Callee::Asm(_), ..))) { return Err("frame-sensitive assembly"); }
        if bi >= region.end && old_exit[bi - region.end] { continue; }
        if b.term.succs().any(|t| t as usize >= f.blocks.len() || region.contains(&(t as usize)) != inside) { return Err("edge across patched region"); }
        if inside && matches!(b.term, Term::Invoke(..) | Term::Resume) {
            let mut t = b.term.clone();
            t.map_succs(target);
            unwind.push((target(bi as u32), t));
        }
    }
    for d in &mut p.dbg {
        if let DbgLoc::Slot(s, off) = d.loc {
            if let Some(&base) = bases.get(s as usize).filter(|&&v| v != DBG_NONE) {
                d.loc = DbgLoc::Mem(base, off);
            } else if let Some(&clone) = layout.slot_map.get(s as usize).filter(|&&s| s != NONE) {
                d.loc = DbgLoc::Slot(clone, off);
            }
        }
    }

    let nv = f.vregs.len();
    let slot = f.slot(layout.record_size, 16);
    let mut new_exits: Vec<_> = (0..old_exit.len()).map(|_| Block { insts: vec![], pos: vec![], term: Term::Unreachable, term_pos: 0, count: -1.0 }).collect();
    p.block();
    let mut leaves = Vec::new();
    for e in &layout.exits {
        let (pack, unpack) = record_exit(f, &mut p, e, slot, &bases, target(e.target));
        new_exits[e.id as usize] = pack;
        leaves.push((e.id, (region.len() + p.blocks.len()) as u32));
        p.blocks.push(unpack);
    }
    let old_exits = f.blocks.split_off(region.end);
    let mut moved = f.blocks.split_off(region.start);
    for b in &mut moved {
        b.term.map_succs(target);
        for i in &mut b.insts {
            if let Inst::SlotAddr(d, s) = *i {
                let base = bases[s as usize];
                if base != DBG_NONE { *i = Inst::Mov(d, base); }
            }
        }
    }
    p.blocks.splice(1..1, moved);
    dispatch(&mut p, 0, &leaves);
    for b in &mut f.blocks {
        b.term.map_succs(|t| if t as usize >= region.end { t - region.len() as u32 } else { t });
    }
    f.blocks.extend(new_exits);
    let mut out = OutlinedPart { part: p, exits: layout.exits, region: layout.region, record_size: layout.record_size, unwind, bases, shared_slots: layout.shared };
    if let Err(why) = verify_outline(f, &out) {
        f.blocks.truncate(region.start);
        for b in &mut f.blocks {
            b.term.map_succs(|t| if t as usize >= region.start { t + region.len() as u32 } else { t });
        }
        let mut original = vec![NONE; out.part.vregs.len() - nv];
        for (s, v) in out.bases.iter().enumerate().filter(|(_, v)| **v != DBG_NONE) { original[v.0 as usize - nv] = s as u32; }
        for mut b in out.part.blocks.drain(1..=region.len()) {
            b.term.map_succs(|t| t - 1 + region.start as u32);
            for i in &mut b.insts {
                if let Inst::Mov(d, a) = *i {
                    if a.0 as usize >= nv { *i = Inst::SlotAddr(d, original[a.0 as usize - nv]); }
                }
            }
            f.blocks.push(b);
        }
        f.blocks.extend(old_exits);
        f.vregs.truncate(nv);
        f.slots.truncate(slot as usize);
        return Err(why);
    }
    // Frozen leaf destinations follow compact_slots' old-slot order. The
    // host side is rediscovered from Part's arguments after its own passes.
    let mut next = 0;
    let slots: Vec<_> = used_slots(&out.part).into_iter().map(|used| {
        if used { next += 1; next - 1 } else { NONE }
    }).collect();
    crate::opt::compact_slots(&mut out.part);
    out.shared_slots.clear();
    for e in &mut out.exits {
        for (_, value) in &mut e.fields {
            if let RecordValue::Fill { dst, .. } = value { *dst = slots[*dst as usize]; }
        }
        for i in &mut e.remakes {
            if let Inst::SlotAddr(_, s) = i { *s = slots[*s as usize]; }
        }
    }
    crate::opt::compact_slots(f);
    Ok(out)
}

// P stays at baseline. Its dispatch blocks are outside the moved region, and
// each unpack leaf ends at its original target, even after the host loses an ID.
fn verify_dispatch(out: &OutlinedPart, b: u32, exits: &[ExitLayout]) -> Result<(), &'static str> {
    let p = &out.part;
    let region = &out.region;
    let bad = "invalid part dispatch";
    let block = p.blocks.get(b as usize).ok_or(bad)?;
    if let [e] = exits {
        let Term::Jump(l) = block.term else { return Err(bad) };
        let leaf = p.blocks.get(l as usize).ok_or(bad)?;
        if !block.insts.is_empty() || l as usize <= region.len() || leaf.term != Term::Jump((e.target as usize - region.start + 1) as u32) {
            return Err(bad);
        }
        // Decode the unpacked fields from their actual loads and stores. Address
        // temporaries may differ; a restore must reach the original slot offset.
        let mut fields = e.fields.iter();
        let mut addr = std::collections::HashMap::from([(p.params[0], (NONE, 0i64))]);
        let mut insts = leaf.insts.strip_suffix(e.remakes.as_slice()).ok_or("missing exit remake")?.iter();
        while let Some(i) = insts.next() {
            let (a, off, kind, reg) = match *i {
                Inst::SlotAddr(a, s) => { addr.insert(a, (s, 0)); continue; }
                Inst::IBinI(IOp::Add, U64, d, a, k) => {
                    let &(s, off) = addr.get(&a).ok_or("invalid unpack address")?;
                    addr.insert(d, (s, off.checked_add(k).ok_or("invalid unpack address")?));
                    continue;
                }
                Inst::Load(m, d, a, off) => (a, i64::from(off), FillKind::Cell(m), d),
                Inst::VLoad(d, a, off) => (a, i64::from(off), FillKind::Vector, d),
                Inst::Copy(d, a, n) => (a, 0, FillKind::Bytes(n), d),
                _ => return Err("invalid unpack instruction"),
            };
            let Some(&(at, ref value)) = fields.next() else { return Err("unexpected record read"); };
            if addr.get(&a) != Some(&(NONE, i64::from(at) - off)) { return Err("invalid record read"); }
            let expected = match *value {
                RecordValue::Register { class, dst, .. } if reg == dst && p.vregs[dst.0 as usize] == class =>
                    record_mem(class).map_or(FillKind::Vector, FillKind::Cell),
                RecordValue::SharedBase(s) if reg == out.bases[s as usize] => FillKind::Cell(Mem::Int(8, false)),
                RecordValue::Fill { kind: want, dst, offset, .. } => {
                    let (to, disp) = match kind {
                        FillKind::Bytes(_) => (reg, 0),
                        _ => match insts.next() {
                            Some(Inst::Store(m, to, off, v)) if *v == reg && kind == FillKind::Cell(*m) => (*to, i64::from(*off)),
                            Some(Inst::VStore(to, off, v)) if *v == reg && kind == FillKind::Vector => (*to, i64::from(*off)),
                            _ => return Err("missing part fill"),
                        },
                    };
                    if addr.get(&to) != Some(&(dst, offset - disp)) { return Err("wrong part fill destination"); }
                    want
                }
                _ => return Err("wrong part register"),
            };
            if kind != expected { return Err("invalid record read"); }
        }
        return if fields.next().is_some() { Err("missing part field") } else { Ok(()) };
    }
    let (left, right) = exits.split_at(exits.len() / 2);
    let ([Inst::ICmpI(Cond::Lt, false, cmp, id, split)], Term::Branch(c, l, r)) = (&block.insts[..], &block.term) else { return Err(bad) };
    if cmp != c || *id != p.params[1] || *split != i64::from(right[0].id) { return Err(bad); }
    verify_dispatch(out, *l, left)?;
    verify_dispatch(out, *r, right)
}

// At the dispatcher's entry only its two arguments may be live. Number only
// values used before a local definition: all other vregs are absent from every
// live-in set, including the host's old (disjoint) register namespace.
fn verify_part_inputs(p: &Func, reach: &[bool]) -> Result<(), &'static str> {
    let mut defined = vec![NONE; p.vregs.len()];
    let mut index = vec![NONE; p.vregs.len()];
    let mut count = 0;
    let mut terms = Vec::new();
    for (bi, b) in p.blocks.iter().enumerate().filter(|(b, _)| reach[*b]) {
        let mut note = |v: VReg, defined: &[u32]| {
            let k = v.0 as usize;
            if defined[k] != bi as u32 && index[k] == NONE && !p.params.contains(&v) {
                index[k] = count;
                count += 1;
            }
        };
        for i in &b.insts {
            for_uses(i, |v| note(v, &defined));
            for_defs(i, |v| defined[v.0 as usize] = bi as u32);
        }
        term_uses(&b.term, &mut terms);
        for &v in &terms { note(v, &defined); }
    }
    let words = (count as usize).div_ceil(64);
    let blocks: Vec<_> = (0..p.blocks.len()).collect();
    let (gen, kill) = gen_kill(p, &blocks, &|v| index[v.0 as usize], words);
    let (live, _) = relax_backend::regalloc::solve_liveness(p, reach, words, &gen, &kill, false);
    if live[..words].iter().any(|&w| w != 0) { return Err("missing part live-in"); }
    Ok(())
}

/// Verify the original extraction, or the surviving calls in an optimised
/// host against the same frozen layouts. Slot/vreg/block numbers in that host
/// may all have changed. Recover the record from the actual Part arguments.
pub fn verify_outline(f: &Func, out: &OutlinedPart) -> Result<(), &'static str> {
    let p = &out.part;
    f.verify().map_err(|_| "invalid host RIR")?;
    p.verify().map_err(|_| "invalid part RIR")?;
    // Includes the unpack leaves: remaking a host address in P would address P's frame.
    if !out.shared_slots.is_empty() && p.blocks.iter().flat_map(|b| &b.insts)
        .any(|i| matches!(i, Inst::SlotAddr(_, s) if out.shared_slots.binary_search(s).is_ok())) {
        return Err("shared slot address in part");
    }
    if p.params.len() != 2 || p.params.iter().any(|v| p.vregs[v.0 as usize] != Cls::I) || p.sret.is_some() ||
        !p.noalias.is_empty() || p.rets != f.rets || p.slots.iter().any(|s| s.born) { return Err("invalid part frame"); }
    verify_dispatch(out, 0, &out.exits)?;
    if p.blocks.is_empty() || f.blocks.is_empty() { return Err("missing entry block"); }
    let pr = relax_backend::regalloc::reachable(p);
    verify_part_inputs(p, &pr)?;
    for (b, t) in &out.unwind {
        if !pr.get(*b as usize).copied().unwrap_or(false) || p.blocks[*b as usize].term != *t { return Err("missing part unwind edge"); }
    }

    let reach = relax_backend::regalloc::reachable(f);
    let addresses = slot_offsets(f);
    let mut pad = vec![false; f.blocks.len()];
    for (_, b) in f.blocks.iter().enumerate().filter(|(b, _)| reach[*b]) {
        if let Term::Invoke(_, cleanup) = b.term { pad[cleanup as usize] = true; }
    }
    let mut record = None;
    let mut sites = vec![None; f.blocks.len()];
    for (bi, b) in f.blocks.iter().enumerate().filter(|(b, _)| reach[*b]) {
        for (ii, i) in b.insts.iter().enumerate() {
            let Inst::Call(Callee::Part(id), args, results) = i else { continue };
            let Some(id) = id else { return Err("part call without an exit ID") };
            let [a] = args[..] else { return Err("invalid part arguments") };
            let ai = a.0 as usize;
            let Some((s, 0)) = addresses[ai] else { return Err("invalid record address"); };
            if f.vregs[ai] != Cls::I
                || !f.slots.get(s as usize).is_some_and(|s| s.align >= 16 && s.size >= out.record_size)
                || record.replace(s).is_some_and(|old| old != s) { return Err("invalid record address"); }
            let index = out.exits.binary_search_by_key(id, |e| e.id).map_err(|_| "unknown exit ID")?;
            if sites[bi].replace(index).is_some() || pad[bi] || matches!(b.term, Term::Invoke(..)) ||
                !results.iter().map(|v| f.vregs[v.0 as usize]).eq(f.rets.iter().copied()) ||
                !relax_backend::tail::returns_results(f, bi, ii + 1, results) { return Err("part call must return exactly and have no pad"); }
        }
    }
    let Some(record) = record else {
        // With no surviving argument, compaction may have erased the record's
        // identity. Elision is safe when no address of a possible record remains.
        for (bi, b) in f.blocks.iter().enumerate() {
            if reach[bi] && b.insts.iter().any(|i| matches!(*i, Inst::SlotAddr(_, s) if f.slots[s as usize].align >= 16 && f.slots[s as usize].size >= out.record_size)) {
                return Err("unidentified record after exit deletion");
            }
        }
        return Ok(());
    };
    let carries = |a: VReg| addresses[a.0 as usize].is_some_and(|(s, _)| s == record);
    let mut uses = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate().filter(|(b, _)| reach[*b]) {
        let mut writes = Vec::new();
        for i in &b.insts {
            let (a, off, n) = match *i {
                Inst::Store(m, a, off, v) if carries(a) && !carries(v) => {
                    let class = match m { Mem::Int(1 | 2 | 4 | 8, _) => Cls::I, Mem::F32 => Cls::F32, Mem::F64 => Cls::F64,
                        _ => return Err("unsupported record store") };
                    if f.vregs[v.0 as usize] != class { return Err("invalid record store class"); }
                    (a, off, relax_backend::passes::mem_size(m) as u32)
                }
                Inst::VStore(a, off, v) if carries(a) && f.vregs[v.0 as usize] == Cls::V128 => (a, off, 16),
                Inst::Copy(d, a, n) if carries(d) && !carries(a) => (d, 0, n),
                Inst::Mov(d, a) | Inst::IBinI(IOp::Add, IntTy { bits: 64, .. }, d, a, _) if carries(a) && carries(d) => continue,
                Inst::Call(Callee::Part(Some(_)), ref args, _) if carries(args[0]) => continue,
                Inst::SlotAddr(d, s) if s == record => {
                    if !carries(d) { return Err("ambiguous record address"); }
                    continue;
                }
                _ => {
                    let mut other = false;
                    for_uses(i, |a| other |= carries(a));
                    if other { return Err("unsupported use of record address"); }
                    continue;
                }
            };
            if sites[bi].is_none() { return Err("record write outside part exit"); }
            let lo = addresses[a.0 as usize].unwrap().1.checked_add(off.into()).and_then(|v| u32::try_from(v).ok()).ok_or("record write out of bounds")?;
            let hi = lo.checked_add(n).filter(|&hi| hi <= out.record_size).ok_or("record write out of bounds")?;
            writes.push((lo, hi));
        }
        term_uses(&b.term, &mut uses);
        if uses.iter().any(|&v| carries(v)) { return Err("record address in terminator"); }
        if let Some(index) = sites[bi] {
            // A widened store or copy can initialise several adjacent fields.
            writes.sort_unstable();
            let mut ranges = writes.into_iter().peekable();
            let mut through = 0;
            for &(lo, ref value) in &out.exits[index].fields {
                let end = lo + value.size_align()?.0;
                through = through.max(lo);
                while through < end {
                    let Some((_, b)) = ranges.next_if(|&(a, _)| a <= through) else { return Err("missing record field"); };
                    through = through.max(b);
                }
            }
        }
    }
    Ok(())
}

/// A body whose copy a live patch has to reach (by an exit or a guard): a patchable slot's
/// (`Func::patch_flag`), or one that holds copies of such (`unguarded_flags`, `guard_calls`).
pub fn patch_reaches(b: &Func) -> bool {
    b.patch_flag.is_some() || !b.unguarded_flags.is_empty() || !b.guard_calls.is_empty()
}

/// Per block, the guard test it ends in, as (fast target, slow side): a `Branch` one of whose
/// targets is a block holding a slow call (`Func::guard_calls`), or a copy version's compare
/// (`copy_ver_compares`) whose other target is a guard test with the same fast target (it leads
/// to its flags' test). And per block, whether it holds a slow call.
///
/// Only a copy version's compare chains: a block that ends in a guard test may hold code of its
/// own before it, and a branch of the program's into it is no guard test. An emulator's bus read
/// `if !plain(addr) { slow = ..; bus.tick = ..; notes.read(addr) }`, `notes.read` an empty
/// patchable body: the callee's prepared body comes with the copy's jumps threaded, so the `if`'s
/// body ends in the copy's guard (its shared copy version compare) whose fast side is the `if`'s
/// join, and the `if` was read as a guard test leading to it: the fast half went straight to the
/// join and lost both stores.
fn guard_tests(f: &Func) -> (Vec<Option<(u32, u32)>>, Vec<bool>) {
    let nb = f.blocks.len();
    let slow: Vec<bool> = (0..nb).map(|b| (0..f.blocks[b].insts.len()).any(|ii| f.guard_call(b, ii))).collect();
    let mut test: Vec<Option<(u32, u32)>> = vec![None; nb];
    if !slow.contains(&true) {
        return (test, slow);
    }
    let ver_cmp = copy_ver_compares(f);
    loop {
        let mut changed = false;
        for b in 0..nb {
            let Term::Branch(c, x, y) = f.blocks[b].term else { continue };
            if test[b].is_some() || x == y {
                continue;
            }
            let chains = ver_cmp[c.0 as usize];
            let leads = |side: u32, fast: u32| slow[side as usize] || chains && test[side as usize].is_some_and(|(t, _)| t == fast);
            let found = if leads(y, x) {
                Some((x, y))
            } else if leads(x, y) {
                Some((y, x))
            } else {
                None
            };
            if found.is_some() {
                test[b] = found;
                changed = true;
            }
        }
        if !changed {
            return (test, slow);
        }
    }
}

/// Per vreg, whether it is a copy version's compare as `opt::inline_site` emits it, its read
/// shared by several guards or not: `w == ver` (or `!=`, or the word itself) of a relaxed load of
/// an `Addr` at a copy version word's offset (the root's word's: every slot's is at the same
/// offset of its tier record). Each of the three has one definition.
fn copy_ver_compares(f: &Func) -> Vec<bool> {
    let nv = f.vregs.len();
    let Some((word, _)) = f.copy_ver else { return vec![false; nv] };
    let mut def: Vec<Option<(usize, usize)>> = vec![None; nv];
    let mut n = vec![0u8; nv];
    for p in &f.params {
        n[p.0 as usize] = 2;
    }
    for (bi, b) in f.blocks.iter().enumerate() {
        for (ii, i) in b.insts.iter().enumerate() {
            for_defs(i, |d| {
                let k = d.0 as usize;
                n[k] = n[k].saturating_add(1);
                def[k] = Some((bi, ii));
            });
        }
    }
    let one = |v: VReg| def[v.0 as usize].filter(|_| n[v.0 as usize] == 1).map(|(bi, ii)| &f.blocks[bi].insts[ii]);
    let read = |w: VReg| match one(w) {
        Some(Inst::AtomicLoad(Mem::Int(8, false), AtomOrd::Relaxed, _, a)) => matches!(one(*a), Some(Inst::Addr(_, s)) if s.off == word.off),
        _ => false,
    };
    (0..nv)
        .map(|k| match one(VReg(k as u32)) {
            Some(Inst::ICmpI(Cond::Eq | Cond::Ne, _, _, w, _)) => read(*w),
            Some(Inst::AtomicLoad(..)) => read(VReg(k as u32)),
            _ => false,
        })
        .collect()
}

/// The blocks `succ` reaches from `entries` in reverse postorder, and each block's rank in it
/// (usize::MAX: not reached). An edge to a block not after its source closes a cycle (a loop's back
/// edge, also in irreducible flow).
fn rpo(succ: &[Vec<u32>], entries: &[u32]) -> (Vec<usize>, Vec<usize>) {
    let nb = succ.len();
    let mut seen = vec![false; nb];
    let mut post = Vec::with_capacity(nb);
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for &e in entries {
        if seen[e as usize] {
            continue;
        }
        seen[e as usize] = true;
        stack.push((e as usize, 0));
        while let Some(top) = stack.last_mut() {
            let b = top.0;
            match succ[b].get(top.1) {
                Some(&s) => {
                    top.1 += 1;
                    if !seen[s as usize] {
                        seen[s as usize] = true;
                        stack.push((s as usize, 0));
                    }
                }
                None => {
                    post.push(b);
                    stack.pop();
                }
            }
        }
    }
    post.reverse();
    let mut rank = vec![usize::MAX; nb];
    for (k, &b) in post.iter().enumerate() {
        rank[b] = k;
    }
    (post, rank)
}

fn preds_in(succ: &[Vec<u32>], order: &[usize]) -> Vec<Vec<u32>> {
    let mut preds = vec![Vec::new(); succ.len()];
    for &b in order {
        for &s in &succ[b] {
            preds[s as usize].push(b as u32);
        }
    }
    preds
}

/// Gen and kill sets of the blocks `blocks` lists over the registers `idx` numbers (u32::MAX: not
/// counted), `words` per block in list order.
fn gen_kill(f: &Func, blocks: &[usize], idx: &dyn Fn(VReg) -> u32, words: usize) -> (Vec<u64>, Vec<u64>) {
    let mut gen = vec![0u64; blocks.len() * words];
    let mut kill = vec![0u64; blocks.len() * words];
    let mut tu = Vec::new();
    for (k, &b) in blocks.iter().enumerate() {
        let w0 = k * words;
        let used = |v: VReg, gen: &mut [u64], kill: &[u64]| {
            let x = idx(v);
            if x != u32::MAX && kill[w0 + x as usize / 64] >> (x % 64) & 1 == 0 {
                gen[w0 + x as usize / 64] |= 1 << (x % 64);
            }
        };
        for i in &f.blocks[b].insts {
            for_uses(i, |v| used(v, &mut gen, &kill));
            for_defs(i, |v| {
                let x = idx(v);
                if x != u32::MAX {
                    kill[w0 + x as usize / 64] |= 1 << (x % 64);
                }
            });
        }
        term_uses(&f.blocks[b].term, &mut tu);
        for &v in &tu {
            used(v, &mut gen, &kill);
        }
    }
    (gen, kill)
}

/// The live-in sets of the blocks `blocks` lists (`at`: a block's place in the list, u32::MAX for
/// one outside it; edges out of the list are not followed), from their `gen_kill` sets: a backward
/// worklist to the fixpoint. (regalloc::solve_liveness walks from the entry block, which a part of
/// the fn after it does not hold.)
fn liveness_in(f: &Func, blocks: &[usize], at: &[u32], words: usize, gen: &[u64], kill: &[u64]) -> Vec<u64> {
    let n = blocks.len();
    let mut live = vec![0u64; n * words];
    if words == 0 {
        return live;
    }
    let mut preds = vec![Vec::new(); n];
    for (k, &b) in blocks.iter().enumerate() {
        for s in f.blocks[b].term.succs() {
            if at[s as usize] != u32::MAX {
                preds[at[s as usize] as usize].push(k as u32);
            }
        }
    }
    let mut queued = vec![true; n];
    let mut work: Vec<u32> = (0..n as u32).collect();
    let mut out = vec![0u64; words];
    while let Some(k) = work.pop() {
        let k = k as usize;
        queued[k] = false;
        out.fill(0);
        for s in f.blocks[blocks[k]].term.succs() {
            let j = at[s as usize];
            if j != u32::MAX {
                for (o, l) in out.iter_mut().zip(&live[j as usize * words..(j as usize + 1) * words]) {
                    *o |= *l;
                }
            }
        }
        let mut grew = false;
        for w in 0..words {
            let v = gen[k * words + w] | (out[w] & !kill[k * words + w]);
            if v != live[k * words + w] {
                live[k * words + w] = v;
                grew = true;
            }
        }
        if grew {
            for &p in &preds[k] {
                if !queued[p as usize] {
                    queued[p as usize] = true;
                    work.push(p);
                }
            }
        }
    }
    live
}

/// How an instruction touches the bytes of the slots a `ByteMap` numbers: it reads them, writes
/// them whole (through an exact address: they are dead before it) or may write them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Acc {
    Read,
    Kill,
    Write,
}

/// The bytes of some slots, numbered: a slot's index by its slot (`cof`, u32::MAX: not one of
/// them), each one's first bit (`bit0`, one past the last at the end), and which slot each
/// register addresses (`tp`), in the fn `f`.
struct ByteMap<'a> {
    f: &'a Func,
    tp: &'a Taint,
    cof: &'a [u32],
    bit0: &'a [usize],
}

impl ByteMap<'_> {
    /// The bytes address `a` names, to `at` as `k`: every numbered slot (an address of several),
    /// one whole (an unknown offset, or no length), or a range of one.
    fn put(&self, at: &mut dyn FnMut(Acc, usize, usize), k: Acc, a: VReg, off: i64, len: i64) {
        let s = self.tp.slot[a.0 as usize];
        if s == NONE {
            return;
        }
        let bit0 = self.bit0;
        if s == MIXED {
            for c in 0..bit0.len() - 1 {
                at(k, bit0[c], bit0[c + 1]);
            }
            return;
        }
        let c = self.cof.get(s as usize).copied().unwrap_or(u32::MAX);
        if c == u32::MAX {
            return;
        }
        let c = c as usize;
        let size = (bit0[c + 1] - bit0[c]) as i64;
        match self.tp.off[a.0 as usize] {
            Some((o, _)) if len > 0 => {
                let (lo, hi) = ((o + off).clamp(0, size), (o + off + len).clamp(0, size));
                at(k, bit0[c] + lo as usize, bit0[c] + hi as usize);
            }
            _ => at(k, bit0[c], bit0[c + 1]),
        }
    }

    /// The byte accesses of one instruction: a typed access at a known offset reads or writes its
    /// bytes, a copy reads its source's and writes its destination's, a call writes the whole of a
    /// result buffer it gets when only the call can reach it (`result_buffer`, and no other
    /// escape here: `Taint::kept`), any other use of an address (a call, an indexed or atomic
    /// access, a store of it) reads and may write the slot whole. A buffer whose address escapes
    /// elsewhere is read by the call as before: the cost is fill bytes, not correctness.
    fn access(&self, i: &Inst, at: &mut dyn FnMut(Acc, usize, usize)) {
        let size = |m: Mem| relax_backend::passes::mem_size(m) as i64;
        if let Some((r, rest)) = result_buffer(self.f, self.tp, i).filter(|(r, _)| !self.tp.kept[self.tp.slot[r.0 as usize] as usize]) {
            for &u in rest {
                self.put(at, Acc::Read, u, 0, 0);
                self.put(at, Acc::Write, u, 0, 0);
            }
            let s = self.tp.slot[r.0 as usize];
            return self.put(at, Acc::Kill, r, 0, self.f.slots[s as usize].size as i64);
        }
        let (a, off, len, v) = match *i {
            Inst::Load(m, _, a, off) => return self.put(at, Acc::Read, a, off as i64, size(m)),
            Inst::VLoad(_, a, off) => return self.put(at, Acc::Read, a, off as i64, 16),
            Inst::Store(m, a, off, v) => (a, off as i64, size(m), Some(v)),
            Inst::VStore(a, off, v) => (a, off as i64, 16, Some(v)),
            Inst::Copy(d, src, n) => {
                self.put(at, Acc::Read, src, 0, n as i64);
                (d, 0, n as i64, None)
            }
            Inst::SlotAddr(..) | Inst::Mov(..) | Inst::IBinI(IOp::Add | IOp::Sub, ..) | Inst::IBin(IOp::Add, ..) => return,
            _ => {
                for_uses(i, |u| {
                    self.put(at, Acc::Read, u, 0, 0);
                    self.put(at, Acc::Write, u, 0, 0);
                });
                return;
            }
        };
        if let Some(v) = v {
            self.put(at, Acc::Read, v, 0, 0);
            self.put(at, Acc::Write, v, 0, 0);
        }
        let exact = self.tp.slot[a.0 as usize] != MIXED && self.tp.off[a.0 as usize].is_some();
        self.put(at, if exact { Acc::Kill } else { Acc::Write }, a, off, len);
    }
}

fn bits_set(w: &mut [u64], lo: usize, hi: usize) {
    for x in lo..hi {
        w[x / 64] |= 1 << (x % 64);
    }
}

fn bits_clear(w: &mut [u64], lo: usize, hi: usize) {
    for x in lo..hi {
        w[x / 64] &= !(1 << (x % 64));
    }
}

/// Gen and kill sets over the slot bytes `m` numbers, of the blocks `blocks` lists (in list order,
/// `words` each): the bytes a block reads before it writes them whole, and those it writes whole.
fn byte_gen_kill(f: &Func, blocks: &[usize], m: &ByteMap, words: usize) -> (Vec<u64>, Vec<u64>) {
    let n = blocks.len();
    let mut gen = vec![0u64; n * words];
    let mut kill = vec![0u64; n * words];
    let mut tu = Vec::new();
    for (k, &b) in blocks.iter().enumerate() {
        let (g, kl) = (&mut gen[k * words..(k + 1) * words], &mut kill[k * words..(k + 1) * words]);
        let mut at = |a: Acc, lo: usize, hi: usize| match a {
            Acc::Read => {
                for x in lo..hi {
                    if kl[x / 64] >> (x % 64) & 1 == 0 {
                        g[x / 64] |= 1 << (x % 64);
                    }
                }
            }
            Acc::Kill => bits_set(kl, lo, hi),
            Acc::Write => {}
        };
        let blk = &f.blocks[b];
        for i in &blk.insts {
            m.access(i, &mut at);
        }
        term_uses(&blk.term, &mut tu);
        for &u in &tu {
            m.put(&mut at, Acc::Read, u, 0, 0);
        }
    }
    (gen, kill)
}

/// The slot bytes `m` numbers that may have been written on a way from the entry to the end of
/// each block of `order` (`preds`: that flow's), `words` per block of the fn: a forward union to
/// the fixpoint.
fn may_written(f: &Func, order: &[usize], preds: &[Vec<u32>], m: &ByteMap, words: usize) -> Vec<u64> {
    let mut out = vec![0u64; f.blocks.len() * words];
    let mut tu = Vec::new();
    for &b in order {
        let g = &mut out[b * words..(b + 1) * words];
        let mut at = |a: Acc, lo: usize, hi: usize| {
            if a != Acc::Read {
                bits_set(g, lo, hi);
            }
        };
        let blk = &f.blocks[b];
        for i in &blk.insts {
            m.access(i, &mut at);
        }
        term_uses(&blk.term, &mut tu);
        for &u in &tu {
            m.put(&mut at, Acc::Write, u, 0, 0);
        }
    }
    loop {
        let mut changed = false;
        for &b in order {
            for &p in &preds[b] {
                for w in 0..words {
                    let more = out[p as usize * words + w] & !out[b * words + w];
                    if more != 0 {
                        out[b * words + w] |= more;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            return out;
        }
    }
}

/// Per block, the union of its successors' sets (`live`: `words` per block).
fn outs(f: &Func, live: &[u64], words: usize) -> Vec<u64> {
    let mut out = vec![0u64; f.blocks.len() * words];
    for (b, blk) in f.blocks.iter().enumerate() {
        for s in blk.term.succs() {
            for w in 0..words {
                out[b * words + w] |= live[s as usize * words + w];
            }
        }
    }
    out
}

/// Position `p` in the patched path (none stays none).
fn patched_pos(p: u64) -> u64 {
    if p == 0 { 0 } else { p | POS_PATCHED }
}

/// Whether an exit ran at the start of block `b` on every path since a frame last may have waited:
/// from the entry (a frame that has not waited may finish old copies, as `opt::unguard_prefix`
/// lets it) or from an exit, through no call and no back edge.
fn ran_at(b: usize, preds: &[Vec<u32>], rank: &[usize], out: &[bool]) -> bool {
    preds[b].iter().all(|&p| rank[p as usize] < rank[b] && out[p as usize])
}

/// Per block of a split body: its exit, as (hit, miss, the index of its word read): the block
/// reads the build's copy version (`Func::copy_ver`) and goes on to `hit` while it holds the
/// value the build read, else to `miss`, the exit's block into the patched path (a compare for
/// inequality branching the other way, as a pass may turn it, is one too).
pub fn exits_of(f: &Func) -> Vec<Option<(u32, u32, usize)>> {
    let Some((word, ver)) = f.copy_ver.filter(|_| f.patched_path) else { return vec![None; f.blocks.len()] };
    let mut addr = vec![false; f.vregs.len()];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Addr(a, s) = i {
                if *s == word {
                    addr[a.0 as usize] = true;
                }
            }
        }
    }
    f.blocks
        .iter()
        .map(|b| {
            let Term::Branch(h, x, y) = b.term else { return None };
            let k = b.insts.iter().rposition(|i| matches!(i, Inst::ICmpI(Cond::Eq | Cond::Ne, false, d, _, k) if *d == h && *k == ver as i64))?;
            let Inst::ICmpI(cond, _, _, w, _) = b.insts[k] else { return None };
            let j = b.insts[..k].iter().rposition(|i| matches!(i, Inst::AtomicLoad(Mem::Int(8, false), AtomOrd::Relaxed, d, a) if *d == w && addr[a.0 as usize]))?;
            Some(if cond == Cond::Eq { (x, y, j) } else { (y, x, j) })
        })
        .collect()
}

/// A split body's fast flow: an exit goes on to its hit only.
fn fast_succs(f: &Func, ex: &[Option<(u32, u32, usize)>]) -> Vec<Vec<u32>> {
    (0..f.blocks.len()).map(|b| match ex[b] {
        Some((hit, _, _)) => vec![hit],
        None => f.blocks[b].term.succs().collect(),
    }).collect()
}

fn is_wait(f: &Func, b: usize, ii: usize) -> bool {
    matches!(f.blocks[b].insts[ii], Inst::Call(..)) && !f.guard_call(b, ii)
}

/// The fast half's flow from the entry, where a frame may have waited: per block, whether an exit
/// ran at its start (`ran_at`), and whether it is in the patched path (an exit's block or after).
/// The second inlining round takes a copy a patch must reach into the fast half only where one
/// ran (`opt::inline_calls_ordered`).
pub fn exit_cover(f: &Func) -> (Vec<bool>, Vec<bool>) {
    let nb = f.blocks.len();
    let ex = exits_of(f);
    let succ = fast_succs(f, &ex);
    let (order, rank) = rpo(&succ, &[0]);
    let preds = preds_in(&succ, &order);
    let mut out = vec![false; nb];
    let mut ran_in = vec![false; nb];
    for &b in &order {
        let mut ran = ran_at(b, &preds, &rank, &out);
        ran_in[b] = ran;
        for ii in 0..f.blocks[b].insts.len() {
            if ex[b].is_some_and(|e| e.2 == ii) {
                ran = true;
            }
            if is_wait(f, b, ii) {
                ran = false;
            }
        }
        out[b] = ran;
    }
    let misses: Vec<u32> = ex.iter().flatten().map(|e| e.1).collect();
    let all: Vec<Vec<u32>> = f.blocks.iter().map(|b| b.term.succs().collect()).collect();
    let (pat, _) = rpo(&all, &misses);
    let mut patched = vec![false; nb];
    for b in pat {
        patched[b] = true;
    }
    (patched, ran_in)
}

/// Unused reads of a guard's words (an 8-byte relaxed load of a symbol's address) and of what
/// they feed (addresses, compares, ors): a test that became a jump leaves its compare. A relaxed
/// load whose value is unused has no effect, so any such load goes.
fn drop_dead_reads(f: &mut Func) {
    let mut tu = Vec::new();
    loop {
        let nv = f.vregs.len();
        let mut uses = vec![0u32; nv];
        let mut addr = vec![false; nv];
        for b in &f.blocks {
            for i in &b.insts {
                for_uses(i, |u| uses[u.0 as usize] += 1);
                if let Inst::Addr(a, _) = i {
                    addr[a.0 as usize] = true;
                }
            }
            term_uses(&b.term, &mut tu);
            for u in &tu {
                uses[u.0 as usize] += 1;
            }
        }
        let dead = |i: &Inst| -> bool {
            let d = match *i {
                Inst::AtomicLoad(Mem::Int(8, false), AtomOrd::Relaxed, d, a) if addr[a.0 as usize] => d,
                Inst::Addr(d, _) | Inst::ICmpI(_, _, d, _, _) | Inst::IBin(IOp::Or, _, d, _, _) => d,
                _ => return false,
            };
            uses[d.0 as usize] == 0
        };
        let mut any = false;
        for b in &mut f.blocks {
            if !b.insts.iter().any(|i| dead(i)) {
                continue;
            }
            let keep: Vec<bool> = b.insts.iter().map(|i| !dead(i)).collect();
            let mut it = keep.iter();
            b.insts.retain(|_| *it.next().unwrap_or(&true));
            if b.pos.len() == keep.len() {
                let mut it = keep.iter();
                b.pos.retain(|_| *it.next().unwrap_or(&true));
            }
            any = true;
        }
        if !any {
            return;
        }
    }
}

/// Which slot each register of the fast half addresses (`blocks`): `slot[v]` (NONE, MIXED for
/// more than one), its offset in it when every definition agrees (`off`, with the integer type
/// of the add that made it), whether some definition is not such an address (`other`), the
/// slots whose address escapes there (anything but a memory access's base or a copy's operand),
/// and of those the ones something may keep a pointer to (`kept`: any escape but the result
/// buffer of a call that only writes it, `result_buffer`).
struct Taint {
    slot: Vec<u32>,
    off: Vec<Option<(i64, IntTy)>>,
    other: Vec<bool>,
    escaped: Vec<bool>,
    kept: Vec<bool>,
}

const NONE: u32 = u32::MAX;
const MIXED: u32 = u32::MAX - 1;

/// A call that gets a result buffer (a result returned in memory) it writes whole and only
/// writes: the call is to a callee in `Func::sret_callees`, its first argument is a lowering's
/// `Slot::ret` at offset 0 (`t`), and no other argument names that slot. The buffer and the
/// other arguments. Whether anything else may keep a pointer to the slot is `Taint::kept`.
fn result_buffer<'a>(f: &Func, t: &Taint, i: &'a Inst) -> Option<(VReg, &'a [VReg])> {
    let Inst::Call(Callee::Fn(id) | Callee::CFn(id, _), args, _) = i else { return None };
    let (&r, rest) = args.split_first().filter(|_| f.sret_callees.contains(id))?;
    let s = t.slot[r.0 as usize];
    (s != NONE && s != MIXED && f.slots[s as usize].ret && matches!(t.off[r.0 as usize], Some((0, _))) && rest.iter().all(|u| !matches!(t.slot[u.0 as usize], x if x == s || x == MIXED))).then_some((r, rest))
}

fn slot_taint(f: &Func, blocks: &[bool]) -> Taint {
    let nv = f.vregs.len();
    let mut t = Taint { slot: vec![NONE; nv], off: vec![None; nv], other: vec![false; nv], escaped: vec![false; f.slots.len()], kept: vec![false; f.slots.len()] };
    // an address definition: (dst, src slot or vreg, offset added)
    fn join(t: &mut Taint, d: VReg, s: u32, off: Option<(i64, IntTy)>) -> bool {
        let d = d.0 as usize;
        let cur = t.slot[d];
        if cur == NONE {
            t.slot[d] = s;
            t.off[d] = off;
            return true;
        }
        if cur == MIXED {
            return false;
        }
        if s == MIXED || s != cur {
            t.escaped[cur as usize] = true;
            if s != MIXED {
                t.escaped[s as usize] = true;
            }
            t.slot[d] = MIXED;
            t.off[d] = None;
            return true;
        }
        if t.off[d].is_some() && t.off[d].map(|x| x.0) != off.map(|x| x.0) {
            t.off[d] = None;
            return true;
        }
        false
    }
    let from = |t: &Taint, a: VReg, k: Option<(i64, IntTy)>| -> Option<(u32, Option<(i64, IntTy)>)> {
        let s = t.slot[a.0 as usize];
        (s != NONE).then(|| (s, match (t.off[a.0 as usize], k) {
            (Some((o, ty0)), Some((k, ty))) => Some((o.wrapping_add(k), if k == 0 { ty0 } else { ty })),
            _ => None,
        }))
    };
    loop {
        let mut changed = false;
        for (bi, b) in f.blocks.iter().enumerate() {
            if !blocks[bi] {
                continue;
            }
            for i in &b.insts {
                let (d, src) = match *i {
                    Inst::SlotAddr(d, s) => (d, Some((s, Some((0, U64))))),
                    Inst::Mov(d, a) => (d, from(&t, a, Some((0, U64)))),
                    Inst::IBinI(IOp::Add, ty, d, a, k) => (d, from(&t, a, Some((k, ty)))),
                    Inst::IBinI(IOp::Sub, ty, d, a, k) => (d, from(&t, a, Some((k.wrapping_neg(), ty)))),
                    Inst::IBin(IOp::Add, _, d, a, x) => {
                        if let Some((s, _)) = from(&t, x, None) {
                            changed |= join(&mut t, d, s, None);
                        }
                        (d, from(&t, a, None).map(|(s, _)| (s, None)))
                    }
                    _ => continue,
                };
                if let Some((s, off)) = src {
                    changed |= join(&mut t, d, s, off);
                }
            }
        }
        if !changed {
            break;
        }
    }
    // definitions that are not a slot's address at a known offset (an indexed one's offset is
    // unknown anyway)
    for &p in &f.params {
        t.other[p.0 as usize] = true;
    }
    loop {
        let mut changed = false;
        for (bi, b) in f.blocks.iter().enumerate() {
            if !blocks[bi] {
                continue;
            }
            for i in &b.insts {
                let bad = match *i {
                    Inst::SlotAddr(..) => continue,
                    Inst::Mov(d, a) | Inst::IBinI(IOp::Add | IOp::Sub, _, d, a, _) => (t.slot[a.0 as usize] == NONE || t.other[a.0 as usize]).then_some(d),
                    _ => {
                        for_defs(i, |d| {
                            if !t.other[d.0 as usize] {
                                t.other[d.0 as usize] = true;
                                changed = true;
                            }
                        });
                        continue;
                    }
                };
                if let Some(d) = bad.filter(|d| !t.other[d.0 as usize]) {
                    t.other[d.0 as usize] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    // escapes
    fn esc(t: &mut Taint, v: VReg) {
        let s = t.slot[v.0 as usize];
        if s != NONE && s != MIXED {
            t.escaped[s as usize] = true;
            t.kept[s as usize] = true;
        }
    }
    let mut tu = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        if !blocks[bi] {
            continue;
        }
        for i in &b.insts {
            if let Some((r, rest)) = result_buffer(f, &t, i) {
                // the callee only writes its result buffer and keeps no pointer to it
                t.escaped[t.slot[r.0 as usize] as usize] = true;
                for &u in rest {
                    esc(&mut t, u);
                }
                continue;
            }
            match *i {
                Inst::Load(..) | Inst::VLoad(..) | Inst::AtomicLoad(..) | Inst::Copy(..) | Inst::SlotAddr(..) | Inst::Mov(..) | Inst::IBinI(IOp::Add | IOp::Sub, ..) | Inst::IBin(IOp::Add, ..) => {}
                Inst::Store(_, _, _, s) | Inst::VStore(_, _, s) | Inst::AtomicStore(_, _, _, s) | Inst::AtomicRmw(_, _, _, _, _, s) => esc(&mut t, s),
                Inst::LoadX(_, _, _, x, _, _) | Inst::VLoadX(_, _, x, _, _) => esc(&mut t, x),
                Inst::StoreX(_, _, x, _, _, s) | Inst::VStoreX(_, x, _, _, s) => {
                    esc(&mut t, x);
                    esc(&mut t, s);
                }
                Inst::AtomicCas(_, _, _, _, _, e, n) => {
                    esc(&mut t, e);
                    esc(&mut t, n);
                }
                _ => for_uses(i, |u| esc(&mut t, u)),
            }
        }
        term_uses(&b.term, &mut tu);
        for &u in &tu {
            esc(&mut t, u);
        }
    }
    t
}

/// How the patched path makes a value it uses where it uses it, so that no exit carries it: a
/// slot's address at a known offset (every definition agrees), or a constant defined once.
#[derive(Clone)]
enum Remake {
    Slot(u32, i64, IntTy),
    Const(Inst),
}

/// Per register of the body (`tp`: its addresses, over all its blocks), how the patched path
/// remakes it, if it can.
fn remakes(f: &Func, tp: &Taint) -> Vec<Option<Remake>> {
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    let mut konst: Vec<Option<Inst>> = vec![None; nv];
    for &p in &f.params {
        ndef[p.0 as usize] += 2;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            if let Inst::Iconst(d, _) | Inst::Fconst(d, _, _) | Inst::VConst(d, _) | Inst::Addr(d, _) = i {
                konst[d.0 as usize] = Some(i.clone());
            }
        }
    }
    (0..nv)
        .map(|v| {
            let s = tp.slot[v];
            if s != NONE && s != MIXED && !tp.other[v] {
                if let Some((o, ty)) = tp.off[v] {
                    return Some(Remake::Slot(s, o, ty));
                }
            }
            if ndef[v] == 1 {
                return konst[v].clone().map(Remake::Const);
            }
            None
        })
        .collect()
}

/// Appends the instructions that remake a value (`Remake`) of class `cls` at position `p`; its
/// new register.
fn remake_into(f: &mut Func, r: &Remake, cls: Cls, insts: &mut Vec<Inst>, pos: &mut Vec<u64>, p: u64) -> VReg {
    match *r {
        Remake::Slot(s, off, ty) => {
            let a = f.vreg(Cls::I);
            insts.push(Inst::SlotAddr(a, s));
            pos.push(p);
            if off == 0 {
                return a;
            }
            let d = f.vreg(Cls::I);
            insts.push(Inst::IBinI(IOp::Add, ty, d, a, off));
            pos.push(p);
            d
        }
        Remake::Const(ref i) => {
            let d = f.vreg(cls);
            insts.push(map_inst(i, &|_| d, 0));
            pos.push(p);
            d
        }
    }
}

/// What an exit placed at a point of the body before the split would carry into the patched path,
/// which copies the code after that point (both sides of each test): the registers live there (but
/// what it remakes, `Remake`), and the bytes of the slots the fast half keeps to itself that are
/// live there and may have been written on a way to it (`order` / `preds`: the fast half's flow).
struct Estimate {
    ridx: Vec<u32>,
    rwords: usize,
    rout: Vec<u64>,
    tp: Taint,
    cof: Vec<u32>,
    bit0: Vec<usize>,
    bwords: usize,
    bout: Vec<u64>,
    wout: Vec<u64>,
}

impl Estimate {
    fn new(f: &Func, order: &[usize], preds: &[Vec<u32>], remake: &[Option<Remake>], tp: Taint) -> Estimate {
        let nb = f.blocks.len();
        let all: Vec<usize> = (0..nb).collect();
        let at: Vec<u32> = (0..nb as u32).collect();
        let ridx: Vec<u32> = remake.iter().enumerate().map(|(v, r)| if r.is_some() { u32::MAX } else { v as u32 }).collect();
        let rwords = ridx.len().div_ceil(64);
        let (gen, kill) = gen_kill(f, &all, &|v| ridx.get(v.0 as usize).copied().unwrap_or(u32::MAX), rwords);
        let rout = outs(f, &liveness_in(f, &all, &at, rwords, &gen, &kill), rwords);
        let mut in_fast = vec![false; nb];
        let mut addressed = vec![false; f.slots.len()];
        for &b in order {
            in_fast[b] = true;
            for i in &f.blocks[b].insts {
                if let Inst::SlotAddr(_, s) = i {
                    addressed[*s as usize] = true;
                }
            }
        }
        let tf = slot_taint(f, &in_fast);
        let mut cof = vec![u32::MAX; f.slots.len()];
        let mut bit0 = vec![0usize];
        for s in 0..f.slots.len() {
            if addressed[s] && !tf.escaped[s] {
                cof[s] = (bit0.len() - 1) as u32;
                bit0.push(bit0[bit0.len() - 1] + f.slots[s].size as usize);
            }
        }
        let bwords = bit0[bit0.len() - 1].div_ceil(64);
        let m = ByteMap { f, tp: &tp, cof: &cof, bit0: &bit0 };
        let (gen, kill) = byte_gen_kill(f, &all, &m, bwords);
        let bout = outs(f, &liveness_in(f, &all, &at, bwords, &gen, &kill), bwords);
        let wout = may_written(f, order, preds, &m, bwords);
        Estimate { ridx, rwords, rout, tp, cof, bit0, bwords, bout, wout }
    }

    /// The estimate at each point of block `b`: before each of its instructions, then before its
    /// term.
    fn costs(&self, f: &Func, b: usize, preds: &[Vec<u32>]) -> Vec<u32> {
        let blk = &f.blocks[b];
        let n = blk.insts.len();
        let (rw, bw) = (self.rwords, self.bwords);
        let m = ByteMap { f, tp: &self.tp, cof: &self.cof, bit0: &self.bit0 };
        // what may have been written before each point
        let mut w = vec![0u64; bw];
        for &p in &preds[b] {
            for k in 0..bw {
                w[k] |= self.wout[p as usize * bw + k];
            }
        }
        let mut wat = vec![0u64; (n + 1) * bw];
        for (ii, i) in blk.insts.iter().enumerate() {
            wat[ii * bw..(ii + 1) * bw].copy_from_slice(&w);
            m.access(i, &mut |a, lo, hi| {
                if a != Acc::Read {
                    bits_set(&mut w, lo, hi);
                }
            });
        }
        wat[n * bw..].copy_from_slice(&w);
        // what is live at each point, from the end back
        let mut r = self.rout[b * rw..(b + 1) * rw].to_vec();
        let mut l = self.bout[b * bw..(b + 1) * bw].to_vec();
        let ri = |v: VReg| self.ridx.get(v.0 as usize).copied().unwrap_or(u32::MAX);
        let mut tu = Vec::new();
        term_uses(&blk.term, &mut tu);
        for &u in &tu {
            let x = ri(u);
            if x != u32::MAX {
                bits_set(&mut r, x as usize, x as usize + 1);
            }
            m.put(&mut |_, lo, hi| bits_set(&mut l, lo, hi), Acc::Read, u, 0, 0);
        }
        let count = |r: &[u64], l: &[u64], w: &[u64]| r.iter().map(|x| x.count_ones()).sum::<u32>() + l.iter().zip(w).map(|(a, b)| (a & b).count_ones()).sum::<u32>();
        let mut cost = vec![0u32; n + 1];
        cost[n] = count(&r[..], &l[..], &wat[n * bw..]);
        for ii in (0..n).rev() {
            let i = &blk.insts[ii];
            for_defs(i, |d| {
                let x = ri(d);
                if x != u32::MAX {
                    bits_clear(&mut r, x as usize, x as usize + 1);
                }
            });
            for_uses(i, |u| {
                let x = ri(u);
                if x != u32::MAX {
                    bits_set(&mut r, x as usize, x as usize + 1);
                }
            });
            m.access(i, &mut |a, lo, hi| {
                if a == Acc::Kill {
                    bits_clear(&mut l, lo, hi);
                }
            });
            m.access(i, &mut |a, lo, hi| {
                if a == Acc::Read {
                    bits_set(&mut l, lo, hi);
                }
            });
            cost[ii] = count(&r[..], &l[..], &wat[ii * bw..(ii + 1) * bw]);
        }
        cost
    }
}

/// Immediate dominators in a flow (`order`: its blocks in reverse postorder from the entry,
/// `rank`: their places, `preds`): the entry's is itself, a block not reached has none
/// (usize::MAX). Cooper, Harvey and Kennedy's iteration.
fn idoms_in(order: &[usize], rank: &[usize], preds: &[Vec<u32>]) -> Vec<usize> {
    let mut idom = vec![usize::MAX; rank.len()];
    let Some(&entry) = order.first() else { return idom };
    idom[entry] = entry;
    loop {
        let mut changed = false;
        for &b in &order[1..] {
            let mut new = usize::MAX;
            for &p in &preds[b] {
                let p = p as usize;
                if idom[p] == usize::MAX {
                    continue;
                }
                if new == usize::MAX {
                    new = p;
                    continue;
                }
                let (mut x, mut y) = (p, new);
                while x != y {
                    while rank[x] > rank[y] {
                        x = idom[x];
                    }
                    while rank[y] > rank[x] {
                        y = idom[y];
                    }
                }
                new = x;
            }
            if new != idom[b] {
                idom[b] = new;
                changed = true;
            }
        }
        if !changed {
            return idom;
        }
    }
}

/// The fast half's flow before the split.
struct Flow<'a> {
    order: &'a [usize],
    rank: &'a [usize],
    preds: &'a [Vec<u32>],
    test: &'a [Option<(u32, u32)>],
}

/// The first point, in the flow's order, that needs an exit none covers, with exits at the points
/// `pts` (per block, the instruction an exit goes before; the block's length: before its term) and
/// at `extra`: a copy site's test (an exit must have run since the stretch began), or a call the
/// second inlining round may take in (`takes`). As (block, index, whether a copy site).
fn first_need(f: &Func, fl: &Flow, takes: &dyn Fn(&Callee) -> bool, pts: &[Vec<usize>], extra: Option<(usize, usize)>, out: &mut [bool]) -> Option<(usize, usize, bool)> {
    out.fill(false);
    for &b in fl.order {
        let mut ran = ran_at(b, fl.preds, fl.rank, out);
        let blk = &f.blocks[b];
        for ii in 0..=blk.insts.len() {
            if pts[b].contains(&ii) || extra == Some((b, ii)) {
                ran = true;
            }
            if ii == blk.insts.len() {
                break;
            }
            if let Inst::Call(c, _, _) = &blk.insts[ii] {
                if !ran && takes(c) {
                    return Some((b, ii, false));
                }
                ran = false;
            }
        }
        if fl.test[b].is_some() && !ran {
            return Some((b, blk.insts.len(), true));
        }
        out[b] = ran;
    }
    None
}

/// Splits a live session's root build after its first inlining round (see the module doc): the
/// first copy site of each stretch needs an exit, the later ones (an exit ran since the stretch
/// began) jump to their copies; a call the second round may take in (a direct call of a body a
/// patch must reach, `patch_reaches`, or an indirect call) in a stretch with no exit needs one too.
/// Each exit sits at the point of its stretch, from where a frame may last have waited to the
/// point that needs it, where it carries the least into the patched path (`Estimate`): a
/// temporary the stretch builds and uses is not live there yet. The patched path is a copy of the
/// code after the exits, with fresh registers; its copy version compares go straight to their
/// flags, and it makes the slot addresses and constants it uses where it uses them (`Remake`).
/// Each exit's block is a parallel copy: a move per other register live into its target, and the
/// cells of the slots the patched path holds a copy of (a slot whose address does not escape in
/// the fast half) that the target reads before it writes them and the fast half may have written
/// on a way to the exit, a load and a store each, so the fast half's slot stays a promotable
/// temporary and only those cells stay live at the exit. None: nothing to split (no guard, no
/// call to cover), or a guard whose shape the tests do not know.
pub fn split_patched_path(f: &mut Func, get: &dyn Fn(u32) -> Option<(*const Func, u8)>) -> Option<SplitStats> {
    split(f, get, None)
}

/// Make the same in-function split, retaining the information needed to extract
/// it. A refusal still leaves the complete split in `f`.
pub fn split_patched_path_with_layout(f: &mut Func, get: &dyn Fn(u32) -> Option<(*const Func, u8)>) -> Option<(SplitStats, Result<SplitLayout, &'static str>)> {
    let mut layout = None;
    let stats = split(f, get, Some(&mut layout))?;
    Some((stats, layout.expect("a completed split captured its layout")))
}

fn split(f: &mut Func, get: &dyn Fn(u32) -> Option<(*const Func, u8)>,
         layout: Option<&mut Option<Result<SplitLayout, &'static str>>>) -> Option<SplitStats> {
    let (word, ver) = f.copy_ver?;
    if f.patched_path || f.blocks.is_empty() {
        return None;
    }
    let (mut test, slow) = guard_tests(f);
    let nb = f.blocks.len();
    let takes = |c: &Callee| match c {
        Callee::Fn(id) => get(*id).is_some_and(|(p, _)| patch_reaches(unsafe { &*p })),
        Callee::Indirect(_) => true,
        _ => false,
    };
    // the fast half's flow: a guard test goes on to its copy
    let succ: Vec<Vec<u32>> = (0..nb).map(|b| match test[b] {
        Some((t, _)) => vec![t],
        None => f.blocks[b].term.succs().collect(),
    }).collect();
    let (order, rank) = rpo(&succ, &[0]);
    if order.iter().any(|&b| slow[b]) {
        return None;
    }
    let preds = preds_in(&succ, &order);
    let mut st = SplitStats::default();
    let fl = Flow { order: &order, rank: &rank, preds: &preds, test: &test };
    let tp_all = slot_taint(f, &vec![true; nb]);
    let remake = remakes(f, &tp_all);
    // each exit at the point of its stretch where it carries the least (`Estimate`), from where a
    // frame may last have waited to the point that needs it: a point that dominates it with no wait
    // point between, so its loop's; ties go to the nearest. The first point that needs an exit,
    // again, until none does
    let mut pts: Vec<Vec<usize>> = vec![Vec::new(); nb];
    let mut out = vec![false; nb];
    first_need(f, &fl, &takes, &pts, None, &mut out)?;
    let est = Estimate::new(f, &order, &preds, &remake, tp_all);
    let idom = idoms_in(&order, &rank, &preds);
    let mut costs: Vec<Option<Vec<u32>>> = vec![None; nb];
    let mut sites = 0u32;
    while let Some(need) = first_need(f, &fl, &takes, &pts, None, &mut out) {
        let (nbk, ni, site) = need;
        // (cost, distance, block, index)
        let mut best: Option<(u32, u32, usize, usize)> = None;
        let (mut d, mut hi, mut dist) = (nbk, ni, 0u32);
        loop {
            if d != nbk && first_need(f, &fl, &takes, &pts, Some((d, f.blocks[d].insts.len())), &mut out) == Some(need) {
                break;
            }
            let lo = f.blocks[d].insts[..hi].iter().rposition(|i| matches!(i, Inst::Call(..))).map_or(0, |k| k + 1);
            let c = costs[d].get_or_insert_with(|| est.costs(f, d, &preds));
            for ii in (lo..=hi).rev() {
                if best.is_none_or(|x| (c[ii], dist) < (x.0, x.1)) {
                    best = Some((c[ii], dist, d, ii));
                }
                dist += 1;
            }
            if lo > 0 || idom[d] == d || idom[d] == usize::MAX {
                break;
            }
            d = idom[d];
            hi = f.blocks[d].insts.len();
        }
        let (_, _, b, ii) = best.expect("the point that needs the exit");
        pts[b].push(ii);
        if site {
            st.site_exits += 1;
            sites += 1;
        } else {
            st.call_exits += 1;
        }
    }
    drop((est, costs, out));
    st.covered = order.iter().filter(|&&b| test[b].is_some()).count() as u32 - sites;
    // each exit's block splits before its point (from the last point of a block back: the tail
    // takes the block's term); its hit and its way into the patched path are the tail:
    // (exit's block, its hit, the block whose copy it enters)
    let mut points: Vec<(usize, usize)> = (0..nb).flat_map(|b| pts[b].iter().map(move |&ii| (b, ii))).collect();
    points.sort_unstable();
    points.dedup();
    let mut exits: Vec<(usize, u32, u32)> = Vec::new();
    let mut term_at: Vec<u32> = (0..nb as u32).collect();
    let mut k = 0;
    while k < points.len() {
        let b = points[k].0;
        let j = k + points[k..].iter().take_while(|x| x.0 == b).count();
        let mut prev: Option<usize> = None;
        for &(_, ii) in points[k..j].iter().rev() {
            let t = f.blocks.len() as u32;
            let blk = &mut f.blocks[b];
            let insts = blk.insts.split_off(ii);
            let pos = blk.pos.split_off(ii.min(blk.pos.len()));
            let term = std::mem::replace(&mut blk.term, Term::Jump(t));
            let tail = Block { insts, pos, term, term_pos: blk.term_pos, count: blk.count };
            f.blocks.push(tail);
            if term_at[b] == b as u32 {
                term_at[b] = t;
            }
            if let Some(p) = prev {
                exits[p].0 = t as usize;
            }
            exits.push((b, t, t));
            prev = Some(exits.len() - 1);
        }
        k = j;
    }
    test.resize(f.blocks.len(), None);
    for b in 0..nb {
        let t = term_at[b] as usize;
        if t != b {
            test[t] = test[b].take();
        }
    }
    // the patched path: what the exits' targets reach, a copy version's compare going straight
    // to its flags' test
    let nb2 = f.blocks.len();
    let version = |test: &[Option<(u32, u32)>], b: usize| test[b].is_some_and(|(_, e)| test[e as usize].is_some());
    let mut newidx = vec![u32::MAX; nb2];
    let mut region: Vec<usize> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for &(_, _, e) in &exits {
        if newidx[e as usize] == u32::MAX {
            newidx[e as usize] = 0;
            stack.push(e as usize);
        }
        while let Some(b) = stack.pop() {
            region.push(b);
            let next: Vec<u32> = if version(&test, b) { vec![test[b].unwrap().1] } else { f.blocks[b].term.succs().collect() };
            for s in next {
                if newidx[s as usize] == u32::MAX {
                    newidx[s as usize] = 0;
                    stack.push(s as usize);
                }
            }
        }
    }
    region.sort_unstable();
    let base = nb2;
    for (k, &b) in region.iter().enumerate() {
        newidx[b] = (base + k) as u32;
    }
    // fresh registers for every one the patched path names
    let nv0 = f.vregs.len();
    let mut need = vec![false; nv0];
    let mut tu = Vec::new();
    for &b in &region {
        for i in &f.blocks[b].insts {
            for_defs(i, |v| need[v.0 as usize] = true);
            for_uses(i, |v| need[v.0 as usize] = true);
        }
        if !version(&test, b) {
            term_uses(&f.blocks[b].term, &mut tu);
            for v in &tu {
                need[v.0 as usize] = true;
            }
        }
    }
    let mut vmap = vec![u32::MAX; nv0];
    let mut inv: Vec<u32> = Vec::new();
    for v in 0..nv0 {
        if need[v] {
            let cls = f.vregs[v];
            vmap[v] = f.vreg(cls).0;
            inv.push(v as u32);
        }
    }
    let cold = |c: f32| if c >= 0.0 { 0.0 } else { -1.0 };
    {
        let m = |v: VReg| VReg(vmap[v.0 as usize]);
        let mut copied = Vec::with_capacity(region.len());
        for &b in &region {
            let src = &f.blocks[b];
            let insts: Vec<Inst> = src.insts.iter().map(|i| map_inst(i, &m, 0)).collect();
            let term = if version(&test, b) {
                Term::Jump(newidx[test[b].unwrap().1 as usize])
            } else {
                let mut t = src.term.clone();
                term_vregs_mut(&mut t, |v| *v = m(*v));
                t.map_succs(|s| newidx[s as usize]);
                t
            };
            st.insts += insts.len() as u32;
            // (its positions carry POS_PATCHED: the pc map tells a frame here from one in the fast half)
            let pos = src.pos.iter().map(|&p| patched_pos(p)).collect();
            copied.push(Block { insts, pos, term, term_pos: patched_pos(src.term_pos), count: cold(src.count) });
        }
        f.blocks.extend(copied);
        for k in 0..f.nonnull.len() {
            let v = f.nonnull[k];
            if vmap[v.0 as usize] != u32::MAX {
                f.nonnull.push(m(v));
            }
        }
        for k in 0..f.ranged.len() {
            let (v, n, lo, hi) = f.ranged[k];
            if vmap[v.0 as usize] != u32::MAX {
                f.ranged.push((m(v), n, lo, hi));
            }
        }
        for k in 0..f.halves.len() {
            let (a, b) = f.halves[k];
            if vmap[a.0 as usize] != u32::MAX && vmap[b.0 as usize] != u32::MAX {
                f.halves.push((m(a), m(b)));
            }
        }
    }
    st.blocks = region.len() as u32;
    // the patched path makes the slot addresses and constants it uses where it uses them
    // (`Remake`): no exit carries one. An instruction that defines one (a copy of an address)
    // is left in the patched path, dead: every use of what it defines is remade too, its own
    // operand included, so no exit block moves that address (`verify_patched_path` checks the
    // exit blocks' moves) and the pipeline's dce drops it
    {
        let rk = |v: VReg| -> Option<&Remake> {
            let x = (v.0 as usize).checked_sub(nv0)?;
            remake.get(*inv.get(x)? as usize)?.as_ref()
        };
        for b in base..base + region.len() {
            let insts = std::mem::take(&mut f.blocks[b].insts);
            let pos = std::mem::take(&mut f.blocks[b].pos);
            let tpos = f.blocks[b].term_pos;
            let mut ni: Vec<Inst> = Vec::with_capacity(insts.len());
            let mut npos: Vec<u64> = Vec::with_capacity(insts.len());
            let mut map: Vec<(VReg, VReg)> = Vec::new();
            for (k, i) in insts.into_iter().enumerate() {
                let p = pos.get(k).copied().unwrap_or(tpos);
                map.clear();
                for_uses(&i, |u| {
                    if rk(u).is_some() && !map.iter().any(|m| m.0 == u) {
                        map.push((u, u));
                    }
                });
                for m in map.iter_mut() {
                    let cls = f.vregs[m.0 .0 as usize];
                    m.1 = remake_into(f, rk(m.0).expect("a remade value"), cls, &mut ni, &mut npos, p);
                }
                ni.push(if map.is_empty() { i } else { map_inst(&i, &|v| map.iter().find(|m| m.0 == v).map_or(v, |m| m.1), 0) });
                npos.push(p);
            }
            let mut term = std::mem::replace(&mut f.blocks[b].term, Term::Unreachable);
            map.clear();
            term_vregs_mut(&mut term, |v| {
                if rk(*v).is_some() && !map.iter().any(|m| m.0 == *v) {
                    map.push((*v, *v));
                }
            });
            for m in map.iter_mut() {
                let cls = f.vregs[m.0 .0 as usize];
                m.1 = remake_into(f, rk(m.0).expect("a remade value"), cls, &mut ni, &mut npos, tpos);
            }
            term_vregs_mut(&mut term, |v| {
                if let Some(m) = map.iter().find(|m| m.0 == *v) {
                    *v = m.1;
                }
            });
            let blk = &mut f.blocks[b];
            blk.term = term;
            blk.insts = ni;
            blk.pos = npos;
        }
    }
    // what is live into each exit's target, in the patched path's own registers
    let np = region.len();
    let pv = inv.len();
    let words = pv.div_ceil(64);
    let pblocks: Vec<usize> = (base..base + np).collect();
    let mut at = vec![u32::MAX; f.blocks.len()];
    for (k, &b) in pblocks.iter().enumerate() {
        at[b] = k as u32;
    }
    let (gen, kill) = gen_kill(f, &pblocks, &|v| (v.0 as usize).checked_sub(nv0).filter(|&x| x < pv).map_or(u32::MAX, |x| x as u32), words);
    let live_in = liveness_in(f, &pblocks, &at, words, &gen, &kill);
    drop((gen, kill, at));
    // each exit: its block of moves into the target, and the word's read at the end of its block
    let x0 = f.blocks.len();
    let mut is_head = vec![false; x0];
    // (each exit's block in turn: its exit's block)
    let mut xhead: Vec<usize> = Vec::with_capacity(exits.len());
    for &(head, hit, entry) in &exits {
        let e2 = newidx[entry as usize] as usize;
        let ek = e2 - base;
        // (an exit before a call: at the call)
        let p = if hit == entry { f.blocks[hit as usize].pos.first().copied().unwrap_or(f.blocks[head].term_pos) } else { f.blocks[head].term_pos };
        let mut insts = Vec::new();
        for x in 0..pv {
            if live_in[ek * words + x / 64] >> (x % 64) & 1 != 0 {
                insts.push(Inst::Mov(VReg((nv0 + x) as u32), VReg(inv[x])));
            }
        }
        let n = insts.len();
        let xb = f.blocks.len() as u32;
        let count = cold(f.blocks[head].count);
        f.blocks.push(Block { insts, pos: vec![patched_pos(p); n], term: Term::Jump(e2 as u32), term_pos: patched_pos(p), count });
        xhead.push(head);
        st.moved += n as u32;
        let (a, w, h) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        let blk = &mut f.blocks[head];
        blk.insts.extend([Inst::Addr(a, word), Inst::AtomicLoad(Mem::Int(8, false), AtomOrd::Relaxed, w, a), Inst::ICmpI(Cond::Eq, false, h, w, ver as i64)]);
        blk.pos.extend([p; 3]);
        blk.term = Term::Branch(h, hit, xb);
        is_head[head] = true;
    }
    drop(live_in);
    // the fast half's other tests go on to their copies
    for b in 0..base {
        if let Some((t, _)) = test[b] {
            if !is_head[b] {
                f.blocks[b].term = Term::Jump(t);
            }
        }
    }
    crate::loops::clear_unreachable(f);
    // the slots: a copy in the patched path for one both halves use (by its address there, or one
    // carried across) whose address the fast half keeps to itself, its cells the target reads
    // filled at each exit
    let fast: Vec<bool> = (0..f.blocks.len()).map(|b| b < base).collect();
    let t = slot_taint(f, &fast);
    let ns = f.slots.len();
    let (mut in_fast, mut in_pat_slot) = (vec![false; ns], vec![false; ns]);
    for (bi, b) in f.blocks.iter().enumerate().take(base + np) {
        for i in &b.insts {
            if let Inst::SlotAddr(_, s) = i {
                if bi < base { in_fast[*s as usize] = true } else { in_pat_slot[*s as usize] = true }
            }
        }
    }
    let mut escaped = t.escaped.clone();
    for b in &f.blocks[x0..] {
        for i in &b.insts {
            if let Inst::Mov(_, v) = *i {
                let s = t.slot[v.0 as usize];
                if s != NONE && s != MIXED {
                    in_pat_slot[s as usize] = true;
                    // (remade from the copy only at a known offset)
                    escaped[s as usize] |= t.off[v.0 as usize].is_none() || t.other[v.0 as usize];
                }
            }
        }
    }
    let mut smap = vec![u32::MAX; ns];
    let mut cloned: Vec<u32> = Vec::new();
    for s in 0..ns {
        if !(in_fast[s] && in_pat_slot[s]) {
            continue;
        }
        if escaped[s] {
            st.shared += 1;
            continue;
        }
        smap[s] = f.slots.len() as u32;
        let c = Slot { born: false, ..f.slots[s].clone() };
        f.slots.push(c);
        cloned.push(s as u32);
    }
    st.cloned = cloned.len() as u32;
    let mut exit_fills = layout.as_ref().map(|_| (0..exits.len()).map(|_| ExitFills::default()).collect::<Vec<_>>());
    if !cloned.is_empty() {
        for b in &mut f.blocks[base..base + np] {
            for i in &mut b.insts {
                if let Inst::SlotAddr(_, s) = i {
                    if smap[*s as usize] != u32::MAX {
                        *s = smap[*s as usize];
                    }
                }
            }
        }
        let nc = cloned.len();
        // a copy's index among them, by the copy's slot
        let mut cof = vec![u32::MAX; f.slots.len()];
        for (k, &s) in cloned.iter().enumerate() {
            cof[smap[s as usize] as usize] = k as u32;
        }
        // the exits' blocks: an address of a copied slot carried across is the copy's, at the
        // same offset (remade there)
        for xb in x0..f.blocks.len() {
            if !matches!(f.blocks[xb].term, Term::Jump(_)) {
                continue;
            }
            let mut moves = Vec::new();
            for i in std::mem::take(&mut f.blocks[xb].insts) {
                let Inst::Mov(d, v) = i else { unreachable!("an exit block holds moves") };
                let s = t.slot[v.0 as usize];
                if s == NONE || s == MIXED || smap[s as usize] == u32::MAX {
                    moves.push(i);
                    continue;
                }
                let (off, ty) = t.off[v.0 as usize].expect("a copied slot's address carried across has a known offset");
                if off == 0 {
                    moves.push(Inst::SlotAddr(d, smap[s as usize]));
                } else {
                    let a = f.vreg(Cls::I);
                    moves.extend([Inst::SlotAddr(a, smap[s as usize]), Inst::IBinI(IOp::Add, ty, d, a, off)]);
                }
            }
            let p = f.blocks[xb].term_pos;
            f.blocks[xb].pos = vec![p; moves.len()];
            f.blocks[xb].insts = moves;
        }
        // which bytes of the copies the patched path reads before it writes them, live into each
        // of its blocks: an exit fills only those (a byte it does not read keeps no fast cell live)
        let mut bit0 = vec![0usize; nc + 1];
        for (k, &s) in cloned.iter().enumerate() {
            bit0[k + 1] = bit0[k] + f.slots[s as usize].size as usize;
        }
        let words = bit0[nc].div_ceil(64);
        let pmask: Vec<bool> = (0..f.blocks.len()).map(|b| b >= base && b < base + np || b >= x0).collect();
        let tp = slot_taint(f, &pmask);
        let pbl: Vec<usize> = (base..base + np).collect();
        let mut at = vec![u32::MAX; f.blocks.len()];
        for (k, &b) in pbl.iter().enumerate() {
            at[b] = k as u32;
        }
        let (gen, kill) = byte_gen_kill(f, &pbl, &ByteMap { f, tp: &tp, cof: &cof, bit0: &bit0 }, words);
        let live = liveness_in(f, &pbl, &at, words, &gen, &kill);
        drop((gen, kill, at, pbl));
        // and the bytes the fast half may have written on a way to each exit: the others hold no
        // value yet (padding, a buffer not filled), so no exit fills them
        let mut fof = vec![u32::MAX; f.slots.len()];
        for (k, &s) in cloned.iter().enumerate() {
            fof[s as usize] = k as u32;
        }
        let fsucc: Vec<Vec<u32>> = (0..f.blocks.len()).map(|b| if b < base { f.blocks[b].term.succs().filter(|&x| (x as usize) < base).collect() } else { Vec::new() }).collect();
        let (forder, _) = rpo(&fsucc, &[0]);
        let fpreds = preds_in(&fsucc, &forder);
        let written = may_written(f, &forder, &fpreds, &ByteMap { f, tp: &t, cof: &fof, bit0: &bit0 }, words);
        drop((fsucc, forder, fpreds, fof));
        // the fast half's cells of each copied slot (its typed accesses at known offsets), whether
        // it has others (an indexed or atomic access: it stays in memory, copied whole) and
        // whether a copy fills it
        let mut cells: Vec<Vec<(i64, Option<Mem>)>> = vec![Vec::new(); nc];
        let mut untyped = vec![false; nc];
        let mut copied_in = vec![false; nc];
        {
            let ci = |v: VReg| -> Option<usize> {
                let s = t.slot[v.0 as usize];
                (s != NONE && s != MIXED && smap[s as usize] != u32::MAX).then(|| cof[smap[s as usize] as usize] as usize)
            };
            for b in &f.blocks[..base] {
                for i in &b.insts {
                    let (a, off, m) = match *i {
                        Inst::Load(m, _, a, off) | Inst::Store(m, a, off, _) => (a, off, Some(m)),
                        Inst::VLoad(_, a, off) | Inst::VStore(a, off, _) => (a, off, None),
                        Inst::Copy(d, _, _) => {
                            if let Some(c) = ci(d) {
                                copied_in[c] = true;
                            }
                            continue;
                        }
                        Inst::LoadX(_, _, a, ..) | Inst::StoreX(_, a, ..) | Inst::VLoadX(_, a, ..) | Inst::VStoreX(a, ..) | Inst::AtomicLoad(_, _, _, a) | Inst::AtomicStore(_, _, a, _) | Inst::AtomicRmw(_, _, _, _, a, _) | Inst::AtomicCas(_, _, _, _, a, _, _) => {
                            if let Some(c) = ci(a) {
                                untyped[c] = true;
                            }
                            continue;
                        }
                        _ => continue,
                    };
                    let Some(c) = ci(a) else { continue };
                    match t.off[a.0 as usize] {
                        Some((o, _)) => cells[c].push((o + off as i64, m)),
                        None => untyped[c] = true,
                    }
                }
            }
        }
        for c in cells.iter_mut() {
            c.sort_unstable_by_key(|&(o, m)| (o, m.map_or(0, |m| relax_backend::passes::mem_size(m))));
            c.dedup();
        }
        let has = |w: &[u64], k: usize| w[k / 64] >> (k % 64) & 1 != 0;
        for xb in x0..f.blocks.len() {
            let Term::Jump(e2) = f.blocks[xb].term else { continue };
            let p = f.blocks[xb].term_pos;
            let ek = e2 as usize - base;
            let h = xhead[xb - x0];
            let lw: Vec<u64> = live[ek * words..(ek + 1) * words].iter().zip(&written[h * words..(h + 1) * words]).map(|(l, w)| l & w).collect();
            let mut fill = Vec::new();
            for (c, &s) in cloned.iter().enumerate() {
                let size = f.slots[s as usize].size as usize;
                let lb: Vec<bool> = (0..size).map(|k| has(&lw, bit0[c] + k)).collect();
                let Some(first) = lb.iter().position(|&x| x) else { continue };
                st.filled += 1;
                st.fill_bytes += lb.iter().filter(|&&x| x).count() as u32;
                let last = lb.iter().rposition(|&x| x).unwrap_or(first);
                let (ad, asrc) = (f.vreg(Cls::I), f.vreg(Cls::I));
                fill.extend([Inst::SlotAddr(ad, smap[s as usize]), Inst::SlotAddr(asrc, s)]);
                let mut covered = vec![false; size];
                if !untyped[c] {
                    // a load and a store per cell the patched path reads: the fast slot's cells
                    // stay its registers (a whole copy keeps it in memory: passes::sroa's copy
                    // limits) and only the cells read stay live at the exit
                    for &(o, m) in &cells[c] {
                        let len = m.map_or(16, |m| relax_backend::passes::mem_size(m)) as usize;
                        let (lo, hi) = (o as usize, o as usize + len);
                        if !lb[lo..hi.min(size)].iter().any(|&x| x) {
                            continue;
                        }
                        if let Some(exits) = &mut exit_fills {
                            exits[xb - x0].values.push(RecordValue::Fill { src: s, dst: smap[s as usize], offset: o,
                                kind: m.map_or(FillKind::Vector, FillKind::Cell) });
                        }
                        match m {
                            Some(m) => {
                                let v = f.vreg(match m { Mem::F32 => Cls::F32, Mem::F64 => Cls::F64, Mem::Int(..) => Cls::I });
                                fill.extend([Inst::Load(m, v, asrc, o as i32), Inst::Store(m, ad, o as i32, v)]);
                            }
                            None => {
                                let v = f.vreg(Cls::V128);
                                fill.extend([Inst::VLoad(v, asrc, o as i32), Inst::VStore(ad, o as i32, v)]);
                            }
                        }
                        covered[lo..hi.min(size)].iter_mut().for_each(|x| *x = true);
                    }
                }
                // bytes no cell names: copied as they are when something may have filled them
                // (an untyped access or a copy; the others the fast half never wrote)
                let mut k = first;
                while k <= last {
                    if !lb[k] || covered[k] || !(untyped[c] || copied_in[c]) {
                        k += 1;
                        continue;
                    }
                    let lo = k;
                    while k <= last && lb[k] && !covered[k] {
                        k += 1;
                    }
                    if let Some(exits) = &mut exit_fills {
                        exits[xb - x0].values.push(RecordValue::Fill { src: s, dst: smap[s as usize], offset: lo as i64,
                            kind: FillKind::Bytes((k - lo) as u32) });
                    }
                    if lo == 0 {
                        fill.push(Inst::Copy(ad, asrc, (k - lo) as u32));
                    } else {
                        let (d2, s2) = (f.vreg(Cls::I), f.vreg(Cls::I));
                        fill.extend([Inst::IBinI(IOp::Add, U64, d2, ad, lo as i64), Inst::IBinI(IOp::Add, U64, s2, asrc, lo as i64), Inst::Copy(d2, s2, (k - lo) as u32)]);
                    }
                }
            }
            if let Some(exits) = &mut exit_fills { exits[xb - x0].insts = fill.len(); }
            fill.extend(std::mem::take(&mut f.blocks[xb].insts));
            let n = fill.len();
            let blk = &mut f.blocks[xb];
            blk.insts = fill;
            blk.pos = vec![p; n];
        }
    }
    drop_dead_reads(f);
    f.patched_path = true;
    if relax_backend::rir::verify_passes_on() {
        if let Err(e) = verify_patched_path(f) {
            panic!("relax: patched path broken in {}: {}\n{}", f.name, e, f.dump());
        }
    }
    if let (Some(out), Some(fills)) = (layout, exit_fills) {
        let shared: Vec<_> = (0..ns).filter(|&s| in_fast[s] && in_pat_slot[s] && smap[s] == NONE).map(|s| s as u32).collect();
        *out = Some(capture_layout(f, base..x0, vmap, smap, &shared, &t, fills));
    }
    Some(st)
}

/// The exits of a split body (`split_patched_path`) and what they lead to: per block, whether it is
/// in the fast half, an exit's block, or in the patched path.
fn halves(f: &Func) -> (Vec<Option<(u32, u32, usize)>>, Vec<bool>, Vec<bool>, Vec<bool>) {
    let nb = f.blocks.len();
    let ex = exits_of(f);
    let succ = fast_succs(f, &ex);
    let (order, _) = rpo(&succ, &[0]);
    let mut fast = vec![false; nb];
    for b in order {
        fast[b] = true;
    }
    let mut xb = vec![false; nb];
    let mut entries = Vec::new();
    for b in 0..nb {
        if let (true, Some((_, miss, _))) = (fast[b], ex[b]) {
            xb[miss as usize] = true;
            entries.extend(f.blocks[miss as usize].term.succs());
        }
    }
    let all: Vec<Vec<u32>> = f.blocks.iter().map(|b| b.term.succs().collect()).collect();
    let (pat_order, _) = rpo(&all, &entries);
    let mut pat = vec![false; nb];
    for b in pat_order {
        pat[b] = true;
    }
    (ex, fast, xb, pat)
}

/// The split's invariants, after it (and in its tests): the halves are apart (the only edges into
/// the patched path leave from exits' blocks, which only exits enter); the fast half holds no
/// guard's slow call; no register is named in both; an exit's block moves (or remakes) every
/// register live into its target and fills the slots the patched path holds a copy of, whose
/// address never crosses.
pub fn verify_patched_path(f: &Func) -> Result<(), String> {
    let (ex, fast, xb, pat) = halves(f);
    let nb = f.blocks.len();
    for b in 0..nb {
        if fast[b] && (xb[b] || pat[b]) || xb[b] && pat[b] {
            return Err(format!("block {} is in two halves (fast {} exit {} patched {})", b, fast[b], xb[b], pat[b]));
        }
        if fast[b] && (0..f.blocks[b].insts.len()).any(|ii| f.guard_call(b, ii)) {
            return Err(format!("the fast half's block {} holds a guard's slow call", b));
        }
        if (fast[b] || xb[b] || pat[b]) && f.blocks[b].pos.iter().chain([&f.blocks[b].term_pos]).any(|&p| p != 0 && pos_patched(p) != !fast[b]) {
            return Err(format!("block {} has a position {} the patched path", b, if fast[b] { "marked in" } else { "not marked in" }));
        }
        if xb[b] {
            let Term::Jump(t) = f.blocks[b].term else { return Err(format!("exit block {} does not jump", b)) };
            if !pat[t as usize] {
                return Err(format!("exit block {} jumps to {}, not into the patched path", b, t));
            }
            if f.blocks[b].insts.iter().any(|i| !matches!(i, Inst::Mov(..) | Inst::SlotAddr(..) | Inst::IBinI(IOp::Add, ..) | Inst::Copy(..) | Inst::Load(..) | Inst::Store(..) | Inst::VLoad(..) | Inst::VStore(..))) {
                return Err(format!("exit block {} holds more than moves and slot copies", b));
            }
        }
    }
    if !ex.iter().enumerate().any(|(b, e)| fast[b] && e.is_some()) {
        return Ok(());
    }
    let nv = f.vregs.len();
    let mut seen = vec![0u8; nv];
    let mut tu = Vec::new();
    for b in 0..nb {
        let side = if fast[b] { 1 } else if pat[b] { 2 } else { continue };
        let blk = &f.blocks[b];
        let mut bad = None;
        let mut note = |v: VReg| {
            seen[v.0 as usize] |= side;
            if seen[v.0 as usize] == 3 {
                bad = Some(v);
            }
        };
        for i in &blk.insts {
            for_defs(i, &mut note);
            for_uses(i, &mut note);
        }
        term_uses(&blk.term, &mut tu);
        for &v in &tu {
            note(v);
        }
        if let Some(v) = bad {
            return Err(format!("register {:?} is named in both halves (block {})", v, b));
        }
    }
    // slots: a copied one is the fast half's only, its copy the patched path's only
    let ns = f.slots.len();
    let mut addressed = vec![0u8; ns];
    for b in 0..nb {
        let side = if fast[b] { 1 } else if pat[b] { 2 } else { continue };
        for i in &f.blocks[b].insts {
            if let Inst::SlotAddr(_, s) = i {
                addressed[*s as usize] |= side;
            }
        }
    }
    let t = slot_taint(f, &fast);
    let mut copied = vec![false; ns];
    for b in (0..nb).filter(|&b| xb[b]) {
        let blk = &f.blocks[b];
        // (an address: its slot and offset; a loaded value: the slot and offset it came from)
        let mut slot_of = std::collections::HashMap::new();
        let mut loaded = std::collections::HashMap::new();
        let pair = |ss: u32, sd: u32| addressed[ss as usize] & 2 == 0 && addressed[sd as usize] & 1 == 0 && f.slots[ss as usize].size == f.slots[sd as usize].size;
        for i in &blk.insts {
            match *i {
                Inst::SlotAddr(a, s) => {
                    slot_of.insert(a, (s, 0i64));
                }
                Inst::IBinI(IOp::Add, _, d, a, k) => {
                    if let Some(&(s, o)) = slot_of.get(&a) {
                        slot_of.insert(d, (s, o + k));
                    }
                }
                Inst::Copy(d, s, n) => {
                    let (Some(&(sd, od)), Some(&(ss, os))) = (slot_of.get(&d), slot_of.get(&s)) else { return Err(format!("exit block {}: a copy not between slots", b)) };
                    if !pair(ss, sd) || od != os || os < 0 || os + n as i64 > f.slots[ss as usize].size as i64 {
                        return Err(format!("exit block {}: slot {} copied to {} is not a fast slot and its patched copy", b, ss, sd));
                    }
                    copied[ss as usize] = true;
                }
                Inst::Load(_, v, a, off) | Inst::VLoad(v, a, off) => {
                    let Some(&(ss, os)) = slot_of.get(&a) else { return Err(format!("exit block {}: a load not of a slot", b)) };
                    loaded.insert(v, (ss, os + off as i64));
                }
                Inst::Store(_, a, off, v) | Inst::VStore(a, off, v) => {
                    let (Some(&(sd, od)), Some(&(ss, os))) = (slot_of.get(&a), loaded.get(&v)) else { return Err(format!("exit block {}: a store not of a slot's loaded cell", b)) };
                    if !pair(ss, sd) || od + off as i64 != os {
                        return Err(format!("exit block {}: a cell of slot {} stored to {} is not a fast slot's cell into its patched copy", b, ss, sd));
                    }
                    copied[ss as usize] = true;
                }
                _ => {}
            }
        }
    }
    for b in (0..nb).filter(|&b| xb[b]) {
        for i in &f.blocks[b].insts {
            if let Inst::Mov(_, v) = *i {
                let s = t.slot[v.0 as usize];
                if s != NONE && s != MIXED && copied[s as usize] {
                    return Err(format!("exit block {}: the address of copied slot {} crosses ({:?})", b, s, v));
                }
            }
        }
    }
    // registers live into an exit's target: each defined in the exit's block
    let mut idx = vec![u32::MAX; nv];
    let mut n = 0usize;
    for v in 0..nv {
        if seen[v] == 2 {
            idx[v] = n as u32;
            n += 1;
        }
    }
    let words = n.div_ceil(64);
    let pblocks: Vec<usize> = (0..nb).filter(|&b| pat[b]).collect();
    let mut at = vec![u32::MAX; nb];
    for (k, &b) in pblocks.iter().enumerate() {
        at[b] = k as u32;
    }
    let (gen, kill) = gen_kill(f, &pblocks, &|v| idx[v.0 as usize], words);
    let live_in = liveness_in(f, &pblocks, &at, words, &gen, &kill);
    for b in (0..nb).filter(|&b| xb[b]) {
        let Term::Jump(t) = f.blocks[b].term else { continue };
        let mut defd = vec![false; n];
        for i in &f.blocks[b].insts {
            for_defs(i, |v| {
                if idx[v.0 as usize] != u32::MAX {
                    defd[idx[v.0 as usize] as usize] = true;
                }
            });
        }
        for x in 0..n {
            if live_in[at[t as usize] as usize * words + x / 64] >> (x % 64) & 1 != 0 && !defd[x] {
                let v = (0..nv).find(|&v| idx[v] == x as u32).unwrap_or(0);
                return Err(format!("exit block {} leaves v{} live into block {} undefined", b, v, t));
            }
        }
    }
    Ok(())
}

/// A split body's exits that an earlier one covers (it ran on every path since a frame last may
/// have waited) become jumps to their hits: the share pass of a split body (`opt::share_copy_ver_reads`),
/// after the second inlining round (a copy it took in left no call between two exits) and after
/// full unrolling (a constant loop's back edge is gone). Their blocks into the patched path, and
/// what only they reach of it, go. The number deleted.
pub fn share_exits(f: &mut Func) -> u32 {
    let ex = exits_of(f);
    if ex.iter().all(|e| e.is_none()) {
        return 0;
    }
    let nb = f.blocks.len();
    let succ = fast_succs(f, &ex);
    let (order, rank) = rpo(&succ, &[0]);
    let preds = preds_in(&succ, &order);
    let mut out = vec![false; nb];
    let mut gone: Vec<usize> = Vec::new();
    for &b in &order {
        let mut ran = ran_at(b, &preds, &rank, &out);
        for ii in 0..f.blocks[b].insts.len() {
            if ex[b].is_some_and(|e| e.2 == ii) {
                if ran {
                    gone.push(b);
                }
                ran = true;
            }
            if is_wait(f, b, ii) {
                ran = false;
            }
        }
        out[b] = ran;
    }
    for &b in &gone {
        let (hit, _, _) = ex[b].expect("an exit");
        f.blocks[b].term = Term::Jump(hit);
    }
    if !gone.is_empty() {
        crate::loops::clear_unreachable(f);
        drop_dead_reads(f);
    }
    gone.len() as u32
}

/// After the second inlining round: the exits whose stretch (from the exit on, to the calls and
/// back edges where a frame may wait) holds no instruction of a copy a patch must reach (an inline
/// context of a slot `patchable` names, or inside one) become jumps. An exit before a call the
/// round did not take is one. The number deleted.
pub fn prune_exits(f: &mut Func, patchable: &dyn Fn(u32) -> bool) -> u32 {
    let ex = exits_of(f);
    if ex.iter().all(|e| e.is_none()) {
        return 0;
    }
    let nb = f.blocks.len();
    let nc = f.inl.len();
    let mut pc = vec![false; nc + 1];
    for c in 1..=nc {
        let parent = pos_ctx(f.inl[c - 1].0) as usize;
        pc[c] = f.inl_slots.get(c - 1).is_none_or(|&s| patchable(s)) || parent >= c || pc[parent];
    }
    let copy = |p: u64| {
        let c = pos_ctx(p) as usize;
        c != 0 && (c > nc || pc[c])
    };
    let succ = fast_succs(f, &ex);
    let (_, rank) = rpo(&succ, &[0]);
    let mut stamp = vec![0u32; nb];
    let mut gone: Vec<usize> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for b in 0..nb {
        let Some((hit, _, _)) = ex[b] else { continue };
        if rank[b] == usize::MAX {
            continue;
        }
        let mark = b as u32 + 1;
        let mut holds = false;
        stack.clear();
        stack.push(hit as usize);
        stamp[hit as usize] = mark;
        'walk: while let Some(x) = stack.pop() {
            let blk = &f.blocks[x];
            for ii in 0..blk.insts.len() {
                if copy(blk.pos.get(ii).copied().unwrap_or(0)) {
                    holds = true;
                    break 'walk;
                }
                if is_wait(f, x, ii) {
                    continue 'walk;
                }
            }
            if copy(blk.term_pos) {
                holds = true;
                break;
            }
            for &s in &succ[x] {
                if rank[s as usize] > rank[x] && stamp[s as usize] != mark {
                    stamp[s as usize] = mark;
                    stack.push(s as usize);
                }
            }
        }
        if !holds {
            gone.push(b);
        }
    }
    for &b in &gone {
        let (hit, _, _) = ex[b].expect("an exit");
        f.blocks[b].term = Term::Jump(hit);
    }
    if !gone.is_empty() {
        crate::loops::clear_unreachable(f);
        drop_dead_reads(f);
    }
    gone.len() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opt::{inline_calls, inline_calls_ordered, InlineLimits, InlineStats, ATTR_ALWAYS, WHY_NO_EXIT, WHY_PATCHED_PATH};

    const FLAG: Sym = Sym { id: 9, off: 16 };
    const SIX_FLAG: Sym = Sym { id: 10, off: 16 };
    const WORD: Sym = Sym { id: 11, off: 24 };
    const M8: Mem = Mem::Int(8, false);
    const M1: Mem = Mem::Int(1, false);

    /// `fn(x) -> x + 1 + 3` (positions in its own body), patchable by `flag`
    fn leaf(name: &str, flag: Option<Sym>) -> Func {
        let mut f = Func::new(name.into(), 0);
        let (x, y) = (f.vreg(Cls::I), f.vreg(Cls::I));
        f.params.push(x);
        f.rets.push(Cls::I);
        f.block();
        f.blocks[0].insts = vec![Inst::IBinI(IOp::Add, I64, y, x, 1), Inst::IBinI(IOp::Add, I64, y, y, 3)];
        f.blocks[0].pos = vec![1, 1];
        f.blocks[0].term = Term::Ret(vec![y]);
        f.blocks[0].term_pos = 1;
        f.patch_flag = flag;
        f
    }

    /// `fn(x) -> leaf(x)`: not patchable, but its body calls a patchable one
    fn mid() -> Func {
        let mut f = Func::new("mid".into(), 0);
        let x = f.vreg(Cls::I);
        f.params.push(x);
        f.rets.push(Cls::I);
        f.block();
        f.blocks[0].insts = vec![Inst::Call(Callee::Fn(1), vec![x], vec![x])];
        f.blocks[0].pos = vec![2];
        f.blocks[0].term = Term::Ret(vec![x]);
        f.blocks[0].term_pos = 2;
        f
    }

    /// A live session's root build: `v = 0; loop { v = calls(v)...; if !c { break } }; v`, with
    /// `before` at the end of the entry block and `each` (and the calls) in the loop
    fn root(before: Vec<Inst>, calls: &[u32], each: Vec<Inst>, v: VReg, f: &mut Func) {
        root_head(before, Vec::new(), calls, each, v, f);
    }

    /// `root`, with `head` in the loop before the calls
    /// A patchable leaf that returns its result in memory: it writes both words of the buffer its
    /// first argument names.
    fn ret_leaf() -> Func {
        let mut f = Func::new("ret_leaf".into(), 0);
        let (r, x) = (f.vreg(Cls::I), f.vreg(Cls::I));
        f.params.extend([r, x]);
        f.block();
        f.blocks[0].insts = vec![Inst::Store(M8, r, 0, x), Inst::Store(M8, r, 8, x)];
        f.blocks[0].pos = vec![1, 1];
        f.blocks[0].term = Term::Ret(Vec::new());
        f.blocks[0].term_pos = 1;
        f.sret = Some(r);
        f.patch_flag = Some(FLAG);
        f
    }
    fn root_head(before: Vec<Inst>, head: Vec<Inst>, calls: &[u32], each: Vec<Inst>, v: VReg, f: &mut Func) {
        for _ in 0..3 {
            f.block();
        }
        let c = f.params[0];
        f.blocks[0].insts = [Inst::Iconst(v, 0)].into_iter().chain(before).collect();
        f.blocks[0].pos = vec![9; f.blocks[0].insts.len()];
        f.blocks[0].term = Term::Jump(1);
        let mut insts = head;
        insts.extend(calls.iter().map(|&id| Inst::Call(Callee::Fn(id), vec![v], vec![v])));
        insts.extend(each);
        f.blocks[1].pos = (0..insts.len() as u64).map(|k| 10 + k).collect();
        f.blocks[1].insts = insts;
        f.blocks[1].term = Term::Branch(c, 1, 2);
        f.blocks[2].term = Term::Ret(vec![v]);
        f.copy_ver = Some((WORD, 3));
    }

    fn new_root() -> (Func, VReg) {
        let mut f = Func::new("root".into(), 0);
        let (c, v) = (f.vreg(Cls::I), f.vreg(Cls::I));
        f.params.push(c);
        f.rets.push(Cls::I);
        (f, v)
    }

    fn count(f: &Func, blocks: &[bool], p: impl Fn(&Inst) -> bool) -> usize {
        f.blocks.iter().enumerate().filter(|(b, _)| blocks[*b]).flat_map(|(_, b)| &b.insts).filter(|i| p(i)).count()
    }

    fn guard_calls_in(f: &Func, blocks: &[bool]) -> usize {
        (0..f.blocks.len()).filter(|&b| blocks[b]).map(|b| (0..f.blocks[b].insts.len()).filter(|&ii| f.guard_call(b, ii)).count()).sum()
    }

    const WAIT: Sym = Sym { id: 12, off: 0 };

    // Three arms with distinct continuations. Each waits, retaining an alias
    // of the shared slot, then runs an inlined patchable call. Reading through
    // the old alias observes whether the continuation wrote the host's slot.
    fn outline_fixture(indirect: bool, pad: bool) -> (Func, SplitLayout) {
        let mut f = Func::new("outline_fixture".into(), 0);
        let (buf, arm, test, a, alias, v, seen, private) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I),
            f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        f.params.extend([buf, arm]);
        if indirect { f.sret = Some(buf); } else { f.rets.push(Cls::I); }
        f.copy_ver = Some((WORD, 3));
        let off = if indirect { 8 } else { 0 };
        let (shared, cell) = (f.slot(8, 8), f.slot(off + 8, 8));
        let cell_addr = f.vreg(Cls::I);
        f.slots[cell as usize].born = true;
        for (local, loc) in [DbgLoc::Slot(shared, 0), DbgLoc::Slot(cell, off as i32), DbgLoc::Regs(vec![arm])].into_iter().enumerate() {
            f.dbg.push(DbgVar { ctx: 0, local: local as u32, loc, k: vec![] });
        }
        for _ in 0..5 { f.block(); }
        // The tuple is constructed by cells, but copied whole both into the captured
        // slot and out to the caller. Its exit field therefore spans all 16 bytes.
        let (builder, tuple) = (f.slot(16, 8), f.slot(16, 8));
        let (build, pair, flag) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        f.blocks[0].insts = vec![Inst::SlotAddr(a, shared), Inst::Store(M8, a, 0, arm),
            Inst::SlotAddr(private, cell), Inst::IBinI(IOp::Add, U64, cell_addr, private, off.into()), Inst::Store(M8, cell_addr, 0, arm),
            Inst::SlotAddr(build, builder), Inst::SlotAddr(pair, tuple), Inst::ICmpI(Cond::Eq, false, flag, arm, 1),
            Inst::Store(M8, build, 0, arm), Inst::Store(M1, build, 8, flag), Inst::Copy(pair, build, 16)];
        f.blocks[0].term = Term::Branch(arm, 1, 2);
        f.blocks[1].insts.push(Inst::ICmpI(Cond::Eq, false, test, arm, 1));
        f.blocks[1].term = Term::Branch(test, 3, 4);
        for b in 2..5 {
            f.blocks[b].insts = vec![Inst::Call(Callee::Host(WAIT), vec![a], vec![alias]),
                Inst::Call(Callee::Fn(1), vec![arm], vec![v]), Inst::IBinI(IOp::Add, U64, v, v, 10 * b as i64),
                if indirect { Inst::AtomicLoad(M8, AtomOrd::Relaxed, seen, cell_addr) } else { Inst::Load(M8, seen, cell_addr, 0) },
                Inst::IBin(IOp::Add, U64, v, v, seen),
                Inst::Copy(buf, pair, 16), Inst::Load(M8, seen, buf, 0), Inst::IBin(IOp::Add, U64, v, v, seen),
                Inst::Load(M1, seen, buf, 8), Inst::IBin(IOp::Add, U64, v, v, seen),
                Inst::Store(M8, a, 0, v), Inst::Load(M8, seen, alias, 0)];
            f.blocks[b].term = if indirect {
                f.blocks[b].insts.extend([Inst::Store(M8, buf, 0, seen), Inst::Store(M8, buf, 8, arm)]);
                Term::Ret(vec![])
            } else { Term::Ret(vec![seen]) };
        }
        if pad {
            let (normal, cleanup) = (f.block(), f.block());
            f.blocks[normal as usize].term = f.blocks[2].term.clone();
            f.blocks[2].insts.push(Inst::Call(Callee::Host(WAIT), vec![a], vec![]));
            f.blocks[2].term = Term::Invoke(normal, cleanup);
            f.blocks[cleanup as usize].insts.push(Inst::Store(M8, a, 0, arm));
            f.blocks[cleanup as usize].term = Term::Resume;
        }
        for (bi, b) in f.blocks.iter_mut().enumerate() {
            b.term_pos = (bi as u64 + 1) * 20;
            b.pos = (1..=b.insts.len() as u64).map(|k| b.term_pos + k).collect();
        }
        let lf = leaf("patchable", Some(FLAG));
        let get = |id| (id == 1).then_some((&lf as *const Func, ATTR_ALWAYS));
        inline_calls(&relax_session::Options::default(), &mut f, 0, &get, &|_| false, &InlineLimits::tier1());
        let (_, layout) = split_patched_path_with_layout(&mut f, &get).unwrap();
        let layout = layout.unwrap();
        assert_eq!(layout.exits.len(), 3, "one exit after each wait");
        (f, layout)
    }

    #[test]
    fn outlined_exits_execute_the_same_continuations_and_returns() {
        #[cfg(target_arch = "aarch64")]
        use relax_backend::arm64 as target;
        #[cfg(not(target_arch = "aarch64"))]
        use relax_backend::x64 as target;
        use relax_backend::exec::Code;
        use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

        let env = target::Env::standalone(false);
        let options = relax_session::Options::default();
        let compile = |f: &Func| crate::native::compile(&options, f, &env).unwrap();
        let mut helper = Func::new("identity".into(), 0);
        let x = helper.vreg(Cls::I);
        helper.params.push(x);
        helper.rets.push(Cls::I);
        helper.block();
        helper.blocks[0].term = Term::Ret(vec![x]);
        let identity = Code::new(&compile(&helper).code).unwrap();
        let mut changed = leaf("replacement", None);
        changed.blocks[0].insts = vec![Inst::IBinI(IOp::Add, U64, x, x, 40)];
        changed.blocks[0].pos = vec![1];
        changed.blocks[0].term = Term::Ret(vec![x]);
        let replacement = compile(&changed);
        assert!(replacement.relocs.is_empty());
        let (word, flag) = (AtomicU64::new(3), AtomicU64::new(0));
        for indirect in [false, true] {
            let (mut f, layout) = outline_fixture(indirect, false);
            assert!(layout.exits.iter().all(|e| e.fields.iter().any(|(_, v)| matches!(v, RecordValue::SharedBase(_)))));
            let fill = if indirect { FillKind::Bytes(8) } else { FillKind::Cell(M8) };
            let off = if indirect { 8 } else { 0 };
            assert!(layout.exits.iter().all(|e| e.fields.iter().any(|(_, v)| matches!(v, RecordValue::Fill { offset, kind, .. } if *kind == fill && *offset == off))));
            assert_eq!(indirect, layout.exits.iter().all(|e| e.fields.iter().any(|(_, v)| matches!(v, RecordValue::Register { result_buffer: true, .. }))));
            assert!(layout.exits.iter().all(|e| e.fields.iter().any(|(_, v)| matches!(v, RecordValue::Fill { kind: FillKind::Bytes(16), .. }))),
                "each exit must carry the entire tuple");
            let mut split = f.clone();
            assert!(relax_backend::passes::sroa(&mut split), "scalarise the in-function split");
            let split = compile(&split);
            let out = extract_patched_part(&mut f, layout, OutlineBody::Native).unwrap();
            assert!(matches!(out.part.dbg[0].loc, DbgLoc::Mem(v, 0) if v != DBG_NONE));
            assert!(matches!(out.part.dbg[1].loc, DbgLoc::Slot(s, o) if s != NONE && i64::from(o) == off));
            assert!(matches!(&out.part.dbg[2].loc, DbgLoc::Regs(v) if v[0] != DBG_NONE && v[0] != f.params[1]));
            assert_eq!(out.part.inl, f.inl);
            assert!(relax_backend::passes::sroa(&mut f), "scalarise the outlined host");
            verify_outline(&f, &out).expect("the scalarised tuple covers its exit field");
            let joined = target::join_part(compile(&f), compile(&out.part)).unwrap();
            for mut image in [split, joined] {
                // Direct slot calls are PC-relative: put the replacement in the
                // same mapping. Pool relocations point at storage owned here.
                let slow = image.code.len().next_multiple_of(16);
                image.code.resize(slow, 0);
                image.code.extend(&replacement.code);
                link(&mut image.code, 0, &image.relocs, &|r| match r {
                    Reloc::Sym(WORD) => word.as_ptr() as u64,
                    Reloc::Sym(FLAG) => flag.as_ptr() as u64,
                    Reloc::Sym(WAIT) => identity.ptr() as u64,
                    Reloc::Call(1) => slow as u64,
                    _ => panic!("unexpected relocation {r:?}"),
                }).unwrap();
                let code = Code::new(&image.code).unwrap();
                for (version, patched, add) in [(3, 0, 4), (4, 0, 4), (4, 1, 40)] {
                    flag.store(patched, Relaxed);
                    word.store(version, Relaxed);
                    for arm in 0..3u64 {
                        let expected = 3 * arm + u64::from(arm == 1) + 10 * (arm + 2) + add;
                        let mut buf = [u64::MAX; 2];
                        if indirect {
                            let call: relax_backend::host_fn_ptr!(fn(*mut u64, u64)) = unsafe { std::mem::transmute(code.ptr()) };
                            unsafe { call(buf.as_mut_ptr(), arm); }
                            assert_eq!(buf, [expected, arm], "indirect result at version {version}, arm {arm}");
                        } else {
                            let call: relax_backend::host_fn_ptr!(fn(*mut u64, u64) -> u64) = unsafe { std::mem::transmute(code.ptr()) };
                            assert_eq!(unsafe { call(buf.as_mut_ptr(), arm) }, expected, "register result at version {version}, arm {arm}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn outline_boundary_survives_pruning_and_rejects_misplaced_writes() {
        let (mut f, mut layout) = outline_fixture(false, false);
        let before = f.clone();
        layout.exits[0].fields.retain(|(_, v)| !matches!(v, RecordValue::SharedBase(_)));
        assert_eq!(extract_patched_part(&mut f, layout, OutlineBody::Native).err(), Some("missing part live-in"));
        assert!(f.same_body_mapped(&mut |s| s, &before, &mut |s| s), "a late refusal restores the split and its metadata");
        let (_, layout) = outline_fixture(false, false);
        let out = extract_patched_part(&mut f, layout, OutlineBody::Native).unwrap();
        verify_outline(&f, &out).unwrap();
        let mut sites: Vec<_> = f.blocks.iter().enumerate().filter_map(|(b, blk)| {
            let Inst::Call(Callee::Part(Some(_)), _, _) = blk.insts.last()? else { return None };
            assert!(matches!(blk.term, Term::Ret(_)));
            Some(b)
        }).collect();
        assert_eq!(sites.len(), 3);
        let site = sites.remove(1);
        let write = f.blocks[site].insts.iter().position(|i| matches!(i, Inst::Store(..))).unwrap();
        let i = f.blocks[site].insts.remove(write);
        assert_eq!(verify_outline(&f, &out), Err("missing record field"));
        f.blocks[site].insts.insert(write, i);
        let Some(Inst::Call(Callee::Part(Some(id)), args, _)) = f.blocks[site].insts.last() else { unreachable!() };
        let record = args[0];
        let exit = out.exits.iter().find(|e| e.id == *id).unwrap();
        let cell = exit.fields.iter().find(|(_, v)| matches!(v, RecordValue::Fill { kind: FillKind::Cell(M8), .. })).unwrap().0;
        let store = f.blocks[site].insts.iter().position(|i| matches!(*i, Inst::Store(M8, a, off, _) if a == record && off == cell as i32)).unwrap();
        let Inst::Store(_, a, off, v) = f.blocks[site].insts[store] else { unreachable!() };
        f.blocks[site].insts[store] = Inst::Store(M1, a, off, v);
        assert_eq!(verify_outline(&f, &out), Err("missing record field"), "narrowing a word Cell must fail coverage");
        f.blocks[site].insts[store] = Inst::Store(M8, a, off, v);

        let dst = f.blocks[site].insts.iter().find_map(|i| match *i { Inst::Copy(d, _, 16) => Some(d), _ => None }).unwrap();
        let build = f.blocks.iter().flat_map(|b| &b.insts).find_map(|i| match *i {
            Inst::Store(M1, a, 8, _) => Some(a), _ => None,
        }).unwrap();
        // Keep the captured Bytes(16) layout frozen, as it is during O3. The same
        // whole-slot copy can carry a tuple, just a variant tag, or no writes at all.
        for defined in [9, 8, 0] {
            let mut host = f.clone();
            for b in &mut host.blocks {
                for k in (0..b.insts.len()).rev() {
                    if matches!(b.insts[k], Inst::Store(_, a, off, _) if a == build && off >= defined) {
                        b.insts.remove(k);
                        b.pos.remove(k);
                    }
                }
            }
            assert!(relax_backend::passes::sroa(&mut host), "promote the {defined}-byte source");
            assert!(!host.blocks.iter().flat_map(|b| &b.insts).any(|i| matches!(i, Inst::Copy(..))), "a whole copy survived promotion");
            verify_outline(&host, &out).expect("every byte of the frozen field should be written");
            let insts = &host.blocks[site].insts;
            let zeros: Vec<_> = insts.iter().filter_map(|i| match *i { Inst::Iconst(v, 0) => Some(v), _ => None }).collect();
            assert_eq!(zeros.len(), 1, "one exit-local zero per copy with gaps");
            let mut filled = Vec::new();
            for i in insts {
                if let Inst::Store(m, a, off, v) = *i {
                    if a == dst && (0..16).contains(&off) && v == zeros[0] {
                        filled.extend(off..off + relax_backend::passes::mem_size(m));
                    }
                }
            }
            assert_eq!(filled, (defined..16).collect::<Vec<_>>(), "only bytes outside the {defined}-byte value are zeroed");
            if defined == 9 {
                for off in [8, 9] {
                    let k = host.blocks[site].insts.iter().position(|i| matches!(*i, Inst::Store(M1, a, o, _) if a == dst && o == off)).unwrap();
                    let i = host.blocks[site].insts.remove(k);
                    let p = host.blocks[site].pos.remove(k);
                    assert_eq!(verify_outline(&host, &out), Err("missing record field"), "the store at {off} is required");
                    host.blocks[site].insts.insert(k, i);
                    host.blocks[site].pos.insert(k, p);
                }
                let buf = host.params[0];
                let writes: Vec<_> = host.blocks.iter().filter_map(|b| {
                    let cells: Vec<_> = b.insts.iter().filter_map(|i| match *i {
                        Inst::Store(m, a, off, _) if a == buf => Some((off, m)), _ => None,
                    }).collect();
                    (!cells.is_empty()).then_some(cells)
                }).collect();
                assert_eq!(writes, vec![vec![(0, M8), (8, M1)]; 3], "each ordinary destination gets just the two data stores");
            }
        }
        let Inst::SlotAddr(_, slot) = f.blocks[site].insts[0] else { panic!("record address") };
        let (a, src, vector) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::V128));
        let data = f.slot(16, 16);
        f.blocks[0].insts.extend([Inst::SlotAddr(a, slot), Inst::SlotAddr(src, data), Inst::VConst(vector, 0)]);
        for moved in [Inst::Store(M8, a, 0, f.params[1]), Inst::Copy(a, src, 16), Inst::VStore(a, 0, vector)] {
            f.blocks[0].insts.push(moved);
            assert_eq!(verify_outline(&f, &out), Err("record write outside part exit"));
            f.blocks[0].insts.pop();
        }
        // Bypass the middle exit at its version check. Block and slot numbers
        // are deliberately free to change afterwards.
        let (head, hit) = f.blocks.iter().enumerate().find_map(|(b, blk)| match blk.term {
            Term::Branch(_, hit, miss) if miss as usize == site => Some((b, hit)), _ => None,
        }).unwrap();
        f.blocks[head].term = Term::Jump(hit);
        crate::loops::clear_unreachable(&mut f);
        relax_backend::passes::compact_vregs(&mut f);
        verify_outline(&f, &out).unwrap();
    }

    #[test]
    fn a_shared_slot_address_remade_at_an_exit_keeps_the_in_function_split() {
        let (mut f, mut layout) = outline_fixture(false, false);
        let addr = Inst::SlotAddr(f.vreg(Cls::I), layout.shared[0]);
        let e = &mut layout.exits[0];
        let b = &mut f.blocks[e.block as usize];
        b.insts.push(addr.clone());
        b.pos.push(b.term_pos);
        e.remakes.push(addr);
        let original = f.dump();
        assert_eq!(extract_patched_part(&mut f, layout, OutlineBody::Native).err(), Some("shared slot address in part"));
        assert_eq!(f.dump(), original, "the refusal restores the split including the host-relative remake");
    }

    #[test]
    fn outlined_unwind_edges_and_resume_belong_to_the_part() {
        let (mut f, layout) = outline_fixture(false, true);
        let mut out = extract_patched_part(&mut f, layout, OutlineBody::Native).unwrap();
        let (site, normal, pad) = out.unwind.iter().find_map(|(b, t)| match *t {
            Term::Invoke(n, u) => Some((*b, n, u)), _ => None,
        }).unwrap();
        assert_eq!(out.part.blocks[pad as usize].term, Term::Resume);
        assert!(out.part.blocks[pad as usize].insts.iter().any(|i| matches!(i, Inst::Store(..))));
        out.part.blocks[site as usize].term = Term::Jump(normal);
        assert_eq!(verify_outline(&f, &out), Err("missing part unwind edge"));
        out.part.blocks[site as usize].term = Term::Invoke(normal, pad);
        let exit = f.blocks.iter().position(|b| matches!(b.insts.last(), Some(Inst::Call(Callee::Part(Some(_)), ..)))).unwrap();
        let normal = f.block();
        f.blocks[normal as usize].term = f.blocks[exit].term.clone();
        f.blocks[exit].term = Term::Invoke(normal, normal);
        assert!(verify_outline(&f, &out).is_err(), "the host exit cannot own a cleanup");
    }

    #[test]
    fn a_split_body_exits_once_per_stretch_into_its_patched_path() {
        // three guarded copies in a loop, a call between the second and the third: the first
        // copy of each stretch (the loop's head, after the call) becomes an exit, the second one
        // jumps to its copy; the patched path holds all three guards (each reads its flag, then
        // calls the slot) and no exit; the fast half holds no guard, no slow call, no flag read
        let options = &relax_session::Options::default();
        let lf = leaf("leaf", Some(FLAG));
        let get = |id: u32| (id == 1).then_some((&lf as *const Func, ATTR_ALWAYS));
        let (mut f, v) = new_root();
        root(Vec::new(), &[1, 1, 5, 1], Vec::new(), v, &mut f);
        assert_eq!(inline_calls(options, &mut f, 0, &get, &|_| false, &InlineLimits::tier1()).len(), 3);
        assert_eq!(f.guard_calls.len(), 3, "{}", f.dump());
        let st = split_patched_path(&mut f, &get).expect("a split");
        assert_eq!((st.site_exits, st.covered, st.call_exits), (2, 1, 0), "{:?} {}", st, f.dump());
        f.verify().unwrap();
        verify_patched_path(&f).unwrap_or_else(|e| panic!("{}\n{}", e, f.dump()));
        let (ex, fast, xb, pat) = halves(&f);
        assert_eq!((0..f.blocks.len()).filter(|&b| fast[b] && ex[b].is_some()).count(), 2, "{}", f.dump());
        assert_eq!(guard_calls_in(&f, &fast), 0, "{}", f.dump());
        assert_eq!(guard_calls_in(&f, &pat), 3, "{}", f.dump());
        let addr = |s: Sym| move |i: &Inst| matches!(i, Inst::Addr(_, x) if *x == s);
        assert_eq!((count(&f, &fast, addr(FLAG)), count(&f, &pat, addr(FLAG))), (0, 3), "{}", f.dump());
        assert_eq!((count(&f, &fast, addr(WORD)), count(&f, &pat, addr(WORD))), (2, 0), "{}", f.dump());
        // the exit's block moves the loop's value into the patched path: without it the verifier
        // finds it live into the target undefined
        let mut broken = f.clone();
        let x = (0..broken.blocks.len()).find(|&b| xb[b] && !broken.blocks[b].insts.is_empty()).expect("an exit block with moves");
        broken.blocks[x].insts.clear();
        broken.blocks[x].pos.clear();
        assert!(verify_patched_path(&broken).is_err_and(|e| e.contains("undefined")));
        // a stretch with no wait point between two exits keeps the first: the call between them
        // gone (as full unrolling removes a copy's constant loop), the share pass deletes the second
        for b in 0..f.blocks.len() {
            if fast[b] {
                if let Some(k) = f.blocks[b].insts.iter().position(|i| matches!(i, Inst::Call(Callee::Fn(5), ..))) {
                    f.blocks[b].insts.remove(k);
                    f.blocks[b].pos.remove(k);
                }
            }
        }
        assert_eq!(share_exits(&mut f), 1, "{}", f.dump());
        f.verify().unwrap();
        verify_patched_path(&f).unwrap_or_else(|e| panic!("{}\n{}", e, f.dump()));
        let (ex, fast, _, pat) = halves(&f);
        assert_eq!((0..f.blocks.len()).filter(|&b| fast[b] && ex[b].is_some()).count(), 1, "{}", f.dump());
        assert_eq!(count(&f, &fast, addr(WORD)), 1, "{}", f.dump());
        assert_eq!(guard_calls_in(&f, &pat), 3, "{}", f.dump());
    }

    #[test]
    fn calls_lowered_at_one_place_are_sites_of_their_own() {
        // The front lowers a match guard of an or-pattern once per alternative: calls of one
        // callee at one source place, here `v = leaf(v)` in a loop and again behind a bounds
        // check, `if v < 8 { v = leaf(v) } else { panic }`. Keyed by place and callee alone, the
        // first copy's slow call took the second call for one of its own: the inliner kept it a
        // call, and the split read the bounds check before it as a guard test whose fast side is
        // the panic, so the fast half panicked on every iteration.
        let options = &relax_session::Options::default();
        let lf = leaf("leaf", Some(FLAG));
        let get = |id: u32| (id == 1).then_some((&lf as *const Func, ATTR_ALWAYS));
        let build = |second: bool| {
            let (mut f, v) = new_root();
            root(Vec::new(), &[1], Vec::new(), v, &mut f);
            let (c, k) = (f.params[0], f.vreg(Cls::I));
            let (ok, panic) = (f.block(), f.block());
            f.blocks[1].insts.push(Inst::ICmpI(Cond::Lt, false, k, v, 8));
            f.blocks[1].pos.push(11);
            f.blocks[1].term = Term::Branch(k, ok, panic);
            if second {
                f.blocks[ok as usize].insts = vec![Inst::Call(Callee::Fn(1), vec![v], vec![v])];
                f.blocks[ok as usize].pos = vec![10];
            }
            f.blocks[ok as usize].term = Term::Branch(c, 1, 2);
            f.blocks[panic as usize].insts = vec![Inst::Call(Callee::Fn(7), vec![], vec![])];
            f.blocks[panic as usize].pos = vec![30];
            f.blocks[panic as usize].term = Term::Unreachable;
            (f, v, k, ok as usize)
        };
        let keeps_check = |f: &Func, k: VReg| {
            let (_, fast, _, _) = halves(f);
            (0..f.blocks.len()).any(|b| fast[b] && matches!(f.blocks[b].term, Term::Branch(x, t, e) if x == k && t != e))
        };
        // both calls inlined, each copy with its own guard; the split keeps the bounds check
        let (mut f, _, k, _) = build(true);
        assert_eq!(inline_calls(options, &mut f, 0, &get, &|_| false, &InlineLimits::tier1()).len(), 2, "each call is a site: {}", f.dump());
        assert_eq!(f.guard_calls.len(), 2, "{}", f.dump());
        split_patched_path(&mut f, &get).expect("a split");
        f.verify().unwrap();
        verify_patched_path(&f).unwrap_or_else(|e| panic!("{}\n{}", e, f.dump()));
        assert!(keeps_check(&f, k), "the fast half keeps the bounds check: {}", f.dump());
        // the second call left a call (as an inliner that priced it out leaves it): a plain call,
        // not the copy's slow call, and the bounds check before it stays a branch
        let (mut f, v, k, ok) = build(false);
        assert_eq!(inline_calls(options, &mut f, 0, &get, &|_| false, &InlineLimits::tier1()).len(), 1);
        f.blocks[ok].insts.insert(0, Inst::Call(Callee::Fn(1), vec![v], vec![v]));
        f.blocks[ok].pos.insert(0, 10);
        assert_eq!(f.guard_calls.len(), 1, "{}", f.dump());
        assert!(!f.guard_call(ok, 0), "a plain call at a guarded copy's place is no slow call: {}", f.dump());
        split_patched_path(&mut f, &get).expect("a split");
        f.verify().unwrap();
        verify_patched_path(&f).unwrap_or_else(|e| panic!("{}\n{}", e, f.dump()));
        assert!(keeps_check(&f, k), "the fast half keeps the bounds check before a plain call: {}", f.dump());
    }

    #[test]
    fn a_branch_into_a_block_that_ends_in_a_guard_stays_a_branch() {
        // `loop { if v >= 8 { *p = v; note(v) } wait(v) }`, `note` an empty patchable body taken
        // in from a prepared callee: the copy's jumps threaded, the `if`'s body ends in the copy's
        // guard (the copy version compare its read shares with the loop's head) whose fast side
        // is the `if`'s join. The `if` is the program's branch: the fast half keeps it and the
        // store, and the guard alone goes. Before, the `if` was read as a guard test leading to
        // the body's and the fast half skipped the body (an emulator's bus read lost its stamp)
        let note = leaf("note", Some(FLAG));
        let get = |id: u32| (id == 1).then_some((&note as *const Func, ATTR_ALWAYS));
        let (mut f, v) = new_root();
        let p = f.vreg(Cls::I);
        f.params.push(p);
        let [k, a, w, h, fa, fw, fc] = std::array::from_fn(|_| f.vreg(Cls::I));
        for _ in 0..7 {
            f.block();
        }
        let c = f.params[0];
        let slow_pos = 20 | relax_backend::rir::POS_GUARD;
        let blocks: [(Vec<Inst>, Term); 7] = [
            (vec![Inst::Iconst(v, 0)], Term::Jump(1)),
            (vec![Inst::Addr(a, WORD), Inst::AtomicLoad(M8, AtomOrd::Relaxed, w, a), Inst::ICmpI(Cond::Eq, false, h, w, 3), Inst::ICmpI(Cond::Lt, false, k, v, 8)], Term::Branch(k, 3, 2)),
            (vec![Inst::Store(M8, p, 0, v)], Term::Branch(h, 3, 5)),
            (vec![Inst::Call(Callee::Fn(5), vec![v], vec![v]), Inst::IBinI(IOp::Add, I64, v, v, 1)], Term::Branch(c, 1, 4)),
            (Vec::new(), Term::Ret(vec![v])),
            (vec![Inst::Addr(fa, FLAG), Inst::AtomicLoad(M8, AtomOrd::Relaxed, fw, fa), Inst::ICmpI(Cond::Eq, false, fc, fw, 0)], Term::Branch(fc, 3, 6)),
            (vec![Inst::Call(Callee::Fn(1), vec![v], vec![])], Term::Jump(3)),
        ];
        for (b, (insts, term)) in blocks.into_iter().enumerate() {
            f.blocks[b].pos = vec![if b == 6 { slow_pos } else { 10 + b as u64 }; insts.len()];
            f.blocks[b].insts = insts;
            f.blocks[b].term = term;
        }
        f.guard_calls.push((slow_pos, 1));
        f.copy_ver = Some((WORD, 3));
        f.verify().unwrap();
        split_patched_path(&mut f, &get).expect("a split");
        f.verify().unwrap();
        verify_patched_path(&f).unwrap_or_else(|e| panic!("{}\n{}", e, f.dump()));
        let (_, fast, _, pat) = halves(&f);
        let branches_on = |blocks: &[bool], x: VReg| (0..f.blocks.len()).any(|b| blocks[b] && matches!(f.blocks[b].term, Term::Branch(y, t, e) if y == x && t != e));
        assert!(branches_on(&fast, k), "the fast half keeps the program's branch: {}", f.dump());
        assert_eq!(count(&f, &fast, |i| matches!(i, Inst::Store(M8, x, 0, _) if *x == p)), 1, "the fast half keeps the store: {}", f.dump());
        assert_eq!((guard_calls_in(&f, &fast), count(&f, &fast, |i| matches!(i, Inst::Addr(_, x) if *x == FLAG))), (0, 0), "and no guard: {}", f.dump());
        assert_eq!(guard_calls_in(&f, &pat), 1, "the patched path keeps it: {}", f.dump());
    }

    #[test]
    fn exits_fill_the_slots_the_patched_path_copies() {
        // a slot both halves address whose address stays in the fast half is the patched path's
        // own copy: an exit fills the cells the patched path reads before it writes them, a load
        // and a store each (a whole copy out of a slot copied in from memory kept the fast slot in
        // memory: passes::sroa), so the fast slot's cells become registers; the patched path makes
        // the slot addresses it uses (a copy's, or a shared slot's: one whose address a call takes)
        // and the exits carry none; a slot the patched path writes before it reads gets no fill,
        // nor does a result buffer its call writes (`Slot::ret`, `Func::sret_callees`); and the
        // exit sits before the loop builds a temporary, so it fills none of it
        let options = &relax_session::Options::default();
        let lf = leaf("leaf", Some(FLAG));
        let rl = ret_leaf();
        let get = |id: u32| match id {
            1 => Some((&lf as *const Func, ATTR_ALWAYS)),
            2 => Some((&rl as *const Func, ATTR_ALWAYS)),
            _ => None,
        };
        let (mut f, v) = new_root();
        let src = f.vreg(Cls::I);
        f.params.push(src);
        let (s, t, u, x, r) = (f.slot(72, 8), f.slot(8, 8), f.slot(8, 8), f.slot(16, 8), f.slot(16, 8));
        f.slots[r as usize].ret = true;
        f.sret_callees.push(2);
        let (a, b, au, wu, xa, wx) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        let (ra, wr) = (f.vreg(Cls::I), f.vreg(Cls::I));
        let w: Vec<VReg> = (0..9).map(|_| f.vreg(Cls::I)).collect();
        let before = vec![Inst::SlotAddr(a, s), Inst::Copy(a, src, 72), Inst::SlotAddr(b, t), Inst::SlotAddr(au, u), Inst::SlotAddr(xa, x), Inst::SlotAddr(ra, r)];
        let head = vec![Inst::Store(M8, xa, 0, v), Inst::Store(M8, xa, 8, v)];
        let mut each: Vec<Inst> = vec![Inst::Call(Callee::Fn(2), vec![ra, v], vec![]), Inst::Load(M8, wr, ra, 8), Inst::IBin(IOp::Add, I64, v, v, wr)];
        for (k, &y) in w.iter().enumerate() {
            each.extend([Inst::Load(M8, y, a, 8 * k as i32), Inst::IBin(IOp::Add, I64, v, v, y)]);
        }
        each.extend([Inst::Call(Callee::Fn(5), vec![b], vec![]), Inst::Store(M8, au, 0, v), Inst::Load(M8, wu, au, 0), Inst::IBin(IOp::Add, I64, v, v, wu)]);
        each.extend([Inst::Load(M8, wx, xa, 8), Inst::IBin(IOp::Add, I64, v, v, wx)]);
        each.extend((0..9).map(|k| Inst::Store(M8, a, 8 * k, v)));
        root_head(before, head, &[1], each, v, &mut f);
        inline_calls(options, &mut f, 0, &get, &|_| false, &InlineLimits::tier1());
        let mut captured = f.clone();
        let st = split_patched_path(&mut f, &get).expect("a split");
        let (stats, layout) = split_patched_path_with_layout(&mut captured, &get).unwrap();
        assert_eq!(stats, st);
        assert_eq!(captured.dump(), f.dump(), "capturing layouts preserves the split");
        let layout = layout.unwrap();
        let [exit] = &layout.exits[..] else { panic!("one exit into the loop") };
        let cells: Vec<_> = exit.fields.iter().filter_map(|(_, value)| match value {
            RecordValue::Fill { src, dst, offset, kind: FillKind::Cell(M8) } if *src == s && *dst == layout.slot_map[s as usize] => Some(*offset),
            _ => None,
        }).collect();
        assert_eq!(cells, (0..72).step_by(8).collect::<Vec<_>>(), "each live cell in the record");
        assert_eq!(exit.fields.iter().filter(|(_, v)| matches!(v, RecordValue::Register { class: Cls::I, .. })).count(), 2);
        assert!(exit.fields.iter().any(|(_, value)| value == &RecordValue::SharedBase(t)), "the escaped slot keeps its identity");
        assert_eq!(layout.record_size, 96, "nine cells, two registers and a shared base");
        assert_eq!((st.cloned, st.shared), (4, 1), "{:?} {}", st, f.dump());
        assert_eq!((st.filled, st.fill_bytes, st.moved), (1, 72, 2), "{:?} {}", st, f.dump());
        f.verify().unwrap();
        verify_patched_path(&f).unwrap_or_else(|e| panic!("{}\n{}", e, f.dump()));
        let (_, fast, xb, pat) = halves(&f);
        let n = f.slots.len() as u32;
        let (s2, u2, x2, r2) = (n - 4, n - 3, n - 2, n - 1);
        assert_eq!((f.slots[s2 as usize].size, f.slots[x2 as usize].size, f.slots[r2 as usize].ret), (72, 16, true));
        let exit_insts: Vec<&Inst> = (0..f.blocks.len()).filter(|&x| xb[x]).flat_map(|x| &f.blocks[x].insts).collect();
        assert!(!exit_insts.iter().any(|i| matches!(i, Inst::Copy(..))), "cells, not a copy: {}", f.dump());
        assert_eq!((exit_insts.iter().filter(|i| matches!(i, Inst::Load(M8, ..))).count(), exit_insts.iter().filter(|i| matches!(i, Inst::Store(M8, ..))).count()), (9, 9), "a load and a store per cell read, none of the temporary: {}", f.dump());
        let addr_of = |blocks: &[bool], sl: u32| count(&f, blocks, |i| matches!(i, Inst::SlotAddr(_, y) if *y == sl));
        assert_eq!((addr_of(&xb, s2), addr_of(&xb, u2), addr_of(&xb, x2), addr_of(&xb, t), addr_of(&xb, r2)), (1, 0, 0, 0, 0), "the fill's address only: {}", f.dump());
        assert!(addr_of(&pat, u2) > 0 && addr_of(&pat, x2) > 0 && addr_of(&pat, t) > 0 && addr_of(&pat, r2) > 0, "the patched path makes its addresses: {}", f.dump());
        assert!(!exit_insts.iter().any(|i| matches!(i, Inst::Mov(_, y) if [a, b, au, xa].contains(y))), "no address crosses: {}", f.dump());
        // the fast half's slot is promoted: its cells are registers there
        assert!(relax_backend::passes::sroa(&mut f), "{}", f.dump());
        relax_backend::passes::dce(&mut f);
        assert_eq!(count(&f, &fast, |i| matches!(i, Inst::SlotAddr(_, y) if *y == s)), 0, "{}", f.dump());

        // a result buffer whose address the patched path also hands to another call (the leaf,
        // taken in in the fast half) may be kept there: its call reads it as before and the exit
        // fills both of its cells
        let (mut f, v) = new_root();
        let r = f.slot(16, 8);
        f.slots[r as usize].ret = true;
        f.sret_callees.push(2);
        let (ra, wr, wl) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        let each = vec![Inst::Call(Callee::Fn(2), vec![ra, v], vec![]), Inst::Load(M8, wr, ra, 8), Inst::IBin(IOp::Add, I64, v, v, wr), Inst::Call(Callee::Fn(1), vec![ra], vec![wl])];
        root_head(vec![Inst::SlotAddr(ra, r)], Vec::new(), &[1], each, v, &mut f);
        inline_calls(options, &mut f, 0, &get, &|_| false, &InlineLimits::tier1());
        let st = split_patched_path(&mut f, &get).expect("a split");
        assert_eq!((st.cloned, st.filled, st.fill_bytes), (1, 1, 16), "{:?} {}", st, f.dump());
        verify_patched_path(&f).unwrap_or_else(|e| panic!("{}\n{}", e, f.dump()));
    }

    #[test]
    fn the_second_round_takes_copies_in_behind_exits_only() {
        // no copy yet: an exit before each call the second round may take in (a patchable body's)
        // where none ran since the loop's head; the round takes the patchable leaf in unguarded
        // behind its exit, keeps a forbidden one a call (its exit then covers no copy and goes),
        // takes a body that is not patchable in anywhere, and keeps the patchable call that body
        // exposes (no exit covers it); the patched path keeps its calls
        let lf = leaf("leaf", Some(FLAG));
        let six = leaf("six", Some(SIX_FLAG));
        let md = mid();
        let get = |id: u32| match id {
            1 => Some((&lf as *const Func, ATTR_ALWAYS)),
            6 => Some((&six as *const Func, ATTR_ALWAYS)),
            7 => Some((&md as *const Func, ATTR_ALWAYS)),
            _ => None,
        };
        let (mut f, v) = new_root();
        root(Vec::new(), &[6, 1, 7], Vec::new(), v, &mut f);
        let st = split_patched_path(&mut f, &get).expect("a split");
        assert_eq!((st.site_exits, st.call_exits), (0, 2), "{:?} {}", st, f.dump());
        verify_patched_path(&f).unwrap_or_else(|e| panic!("{}\n{}", e, f.dump()));
        let options = &relax_session::Options { opt: vec!["-inl@6".into()], ..Default::default() };
        let mut stats = InlineStats::remarks();
        let took = inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &|_, _| false, &|_| false, &InlineLimits::tier1(), None, Some(&mut stats));
        let mut took: Vec<u32> = took.iter().map(|x| x.0).collect();
        took.sort_unstable();
        assert_eq!(took, vec![1, 7], "{}", f.dump());
        assert!(f.guard_calls.is_empty() && f.unguarded_flags == vec![FLAG], "{:?} {}", f.unguarded_flags, f.dump());
        let why = |w: u8| stats.remarks.iter().filter(|r| r.why == w).count();
        assert_eq!(why(WHY_NO_EXIT), 1, "{}", f.dump());
        assert!(why(WHY_PATCHED_PATH) >= 3, "{}", f.dump());
        assert_eq!(prune_exits(&mut f, &|s| s == 1 || s == 6), 1, "{}", f.dump());
        f.verify().unwrap();
        verify_patched_path(&f).unwrap_or_else(|e| panic!("{}\n{}", e, f.dump()));
        let (ex, fast, _, pat) = halves(&f);
        assert_eq!((0..f.blocks.len()).filter(|&b| fast[b] && ex[b].is_some()).count(), 1, "{}", f.dump());
        let calls = |blocks: &[bool], id: u32| count(&f, blocks, |i| matches!(i, Inst::Call(Callee::Fn(x), ..) if *x == id));
        assert_eq!((calls(&fast, 6), calls(&fast, 1), calls(&fast, 7)), (1, 1, 0), "{}", f.dump());
        assert_eq!((calls(&pat, 6), calls(&pat, 1), calls(&pat, 7)), (1, 1, 1), "{}", f.dump());
    }
}
