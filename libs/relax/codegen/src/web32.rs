//! The wasm32 web target's execution on this machine (local/agent_state/wasm/web32.DESIGN.md):
//! a program built for `wasm32-unknown-unknown` has 4-byte pointers, and every pointer value is a
//! 32-bit offset into the linear memory's 4 GB window at `base` (zone_alloc/web.rs), where
//! everything the program can address lives: its code and data (the unit's reservation), its
//! statics and thread-local blocks, its shadow stacks and its heap. `transform` makes a function's
//! RIR run that way, after the wasm export's capture and before native code generation
//! (native.rs): only the program unit's functions (`Cfg::syms`, its symbol table) take it.
//! - every load, store, copy, atomic and vector access goes through `base + zext32(p)`;
//! - the instructions that make an address (a code or data symbol, a static, a thread-local, a
//!   function's thunk) make its offset;
//! - stack slots move to the thread's shadow stack in the window (its pointer and limit are two
//!   words of the thread-local block, `Cfg::ss`; `Cfg::grow` gives a thread its stack);
//! - an indirect call's target is `base + zext32(p)`.
//! Host functions take and return offsets (the web import ABI): the runtime names their W32
//! versions, so calls pass through unchanged.

use relax_backend::rir::{Block, Callee, Cls, Cond, Conv, Func, IOp, Inst, Mem, Sym, SymClass, Term, VReg, U32, U64};
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug)]
pub struct Cfg {
    /// the program unit's symbol value table (`Env::syms`): only its functions are transformed
    pub syms: u64,
    /// the window's base (4 GB aligned; 0 on Linux)
    pub base: u64,
    /// thread-local block offset of the shadow stack words: [pointer, limit] (native addresses)
    pub ss: Sym,
    /// runtime `fn(frame: u64) -> u64`: the new stack pointer after reserving `frame` bytes when
    /// the thread's shadow stack has no room (allocates it on first use, panics on overflow)
    pub grow: Sym,
}

static CFG: OnceLock<Cfg> = OnceLock::new();

/// Names the W32 program unit (once per process: one program world runs as wasm32).
pub fn configure(c: Cfg) -> bool {
    CFG.set(c).is_ok()
}

/// The configuration when the code being compiled (its symbol table) is the W32 program's.
pub fn applies(syms: u64) -> Option<Cfg> {
    CFG.get().copied().filter(|c| c.syms == syms && syms != 0)
}

/// The W32 configuration of this process, if any.
pub fn cfg() -> Option<Cfg> {
    CFG.get().copied()
}

/// Rewrites `f` for W32 execution (see the module header). `sym_class`: the unit's class table.
pub fn transform(f: &mut Func, c: &Cfg, sym_class: u64) -> Result<(), String> {
    let n = f.blocks.len() as u32;
    let pos0 = f.blocks.first().and_then(|b| b.pos.first().copied()).unwrap_or(0);
    // shadow frame: the slots laid out in order, 16-byte aligned in all
    let mut offs = Vec::with_capacity(f.slots.len());
    let mut frame: u64 = 0;
    for s in &f.slots {
        let a = s.align.max(1) as u64;
        frame = (frame + a - 1) / a * a;
        offs.push(frame);
        frame += s.size as u64;
    }
    let frame = (frame + 15) & !15;
    let has_frame = !f.slots.is_empty();
    let base = f.vreg(Cls::I);
    let t_ss = f.vreg(Cls::I);
    let ssp0 = f.vreg(Cls::I);
    let nsp = f.vreg(Cls::I);
    // new block 0: the prologue; the old blocks shift by one
    for b in f.blocks.iter_mut() {
        b.term.map_succs(|t| t + 1);
    }
    let mut b0 = Block { insts: Vec::new(), pos: Vec::new(), term: Term::Jump(1), term_pos: pos0, count: -1.0 };
    let push0 = |b: &mut Block, i: Inst| {
        b.insts.push(i);
        b.pos.push(pos0);
    };
    push0(&mut b0, Inst::Iconst(base, c.base as i64));
    let mut extra: Vec<Block> = Vec::new();
    if has_frame {
        let lim = f.vreg(Cls::I);
        let need = f.vreg(Cls::I);
        let short = f.vreg(Cls::I);
        let fr = f.vreg(Cls::I);
        push0(&mut b0, Inst::TlsAddr(t_ss, c.ss));
        push0(&mut b0, Inst::Load(Mem::Int(8, false), ssp0, t_ss, 0));
        push0(&mut b0, Inst::Load(Mem::Int(8, false), lim, t_ss, 8));
        push0(&mut b0, Inst::IBinI(IOp::Add, U64, need, lim, frame as i64));
        // no room (or no stack yet: both words 0): the runtime's slow path
        push0(&mut b0, Inst::ICmp(Cond::Lt, false, short, ssp0, need));
        push0(&mut b0, Inst::IBinI(IOp::Sub, U64, nsp, ssp0, frame as i64));
        let slow = n + 1;
        let cont = n + 2;
        b0.term = Term::Branch(short, slow, cont);
        let got = f.vreg(Cls::I);
        extra.push(Block {
            insts: vec![Inst::Iconst(fr, frame as i64), Inst::Call(Callee::Host(c.grow), vec![fr], vec![got]), Inst::Mov(nsp, got)],
            pos: vec![pos0, pos0, pos0],
            term: Term::Jump(cont),
            term_pos: pos0,
            count: -1.0,
        });
        extra.push(Block { insts: vec![Inst::Store(Mem::Int(8, false), t_ss, 0, nsp)], pos: vec![pos0], term: Term::Jump(1), term_pos: pos0, count: -1.0 });
    }
    let blocks = std::mem::take(&mut f.blocks);
    let mut out: Vec<Block> = Vec::with_capacity(blocks.len() + 3);
    out.push(b0);
    for b in blocks {
        let mut insts = Vec::with_capacity(b.insts.len() * 2);
        let mut pos = Vec::with_capacity(b.insts.len() * 2);
        // masked bases of this block (until the vreg is written again)
        let mut cache: HashMap<u32, VReg> = HashMap::new();
        for (k, inst) in b.insts.into_iter().enumerate() {
            let p = b.pos.get(k).copied().unwrap_or(pos0);
            let mut defs: Vec<VReg> = Vec::new();
            relax_backend::regalloc::for_defs(&inst, |d| defs.push(d));
            let mut mask = |f: &mut Func, insts: &mut Vec<Inst>, pos: &mut Vec<u64>, v: VReg| -> VReg {
                if let Some(&m) = cache.get(&v.0) {
                    return m;
                }
                let z = f.vreg(Cls::I);
                insts.push(Inst::Conv(Conv::IntToInt(U32), z, v));
                pos.push(p);
                let m = if c.base == 0 {
                    z
                } else {
                    let m = f.vreg(Cls::I);
                    insts.push(Inst::IBin(IOp::Add, U64, m, z, base));
                    pos.push(p);
                    m
                };
                cache.insert(v.0, m);
                m
            };
            // an address-making instruction: `d` = its 32-bit offset
            let offset = |insts: &mut Vec<Inst>, pos: &mut Vec<u64>, i: Inst, d: VReg| {
                insts.push(i);
                pos.push(p);
                insts.push(Inst::Conv(Conv::IntToInt(U32), d, d));
                pos.push(p);
            };
            match inst {
                Inst::SlotAddr(d, s) => {
                    offset(&mut insts, &mut pos, Inst::IBinI(IOp::Add, U64, d, nsp, offs[s as usize] as i64), d);
                }
                Inst::Addr(d, sym) => match SymClass::of(sym_class, sym.id) {
                    SymClass::Near | SymClass::Domain => offset(&mut insts, &mut pos, Inst::Addr(d, sym), d),
                    // a per-run number, or a host address (a call target held as a value)
                    SymClass::Imm | SymClass::Far => {
                        insts.push(Inst::Addr(d, sym));
                        pos.push(p);
                    }
                },
                Inst::FnAddr(d, id) => offset(&mut insts, &mut pos, Inst::FnAddr(d, id), d),
                Inst::TlsAddr(d, sym) => offset(&mut insts, &mut pos, Inst::TlsAddr(d, sym), d),
                Inst::Load(m, d, a, o) => {
                    let a = mask(f, &mut insts, &mut pos, a);
                    insts.push(Inst::Load(m, d, a, o));
                    pos.push(p);
                }
                Inst::Store(m, a, o, s) => {
                    let a = mask(f, &mut insts, &mut pos, a);
                    insts.push(Inst::Store(m, a, o, s));
                    pos.push(p);
                }
                Inst::LoadX(m, d, a, x, sh, o) => {
                    let a = mask(f, &mut insts, &mut pos, a);
                    insts.push(Inst::LoadX(m, d, a, x, sh, o));
                    pos.push(p);
                }
                Inst::StoreX(m, a, x, sh, o, s) => {
                    let a = mask(f, &mut insts, &mut pos, a);
                    insts.push(Inst::StoreX(m, a, x, sh, o, s));
                    pos.push(p);
                }
                Inst::VLoad(d, a, o) => {
                    let a = mask(f, &mut insts, &mut pos, a);
                    insts.push(Inst::VLoad(d, a, o));
                    pos.push(p);
                }
                Inst::VStore(a, o, s) => {
                    let a = mask(f, &mut insts, &mut pos, a);
                    insts.push(Inst::VStore(a, o, s));
                    pos.push(p);
                }
                Inst::VLoadX(d, a, x, sh, o) => {
                    let a = mask(f, &mut insts, &mut pos, a);
                    insts.push(Inst::VLoadX(d, a, x, sh, o));
                    pos.push(p);
                }
                Inst::VStoreX(a, x, sh, o, s) => {
                    let a = mask(f, &mut insts, &mut pos, a);
                    insts.push(Inst::VStoreX(a, x, sh, o, s));
                    pos.push(p);
                }
                Inst::VGather(d, a, x) => {
                    let a = mask(f, &mut insts, &mut pos, a);
                    insts.push(Inst::VGather(d, a, x));
                    pos.push(p);
                }
                Inst::VTbl(d, a, x, k) => {
                    let a = mask(f, &mut insts, &mut pos, a);
                    insts.push(Inst::VTbl(d, a, x, k));
                    pos.push(p);
                }
                Inst::AtomicLoad(m, o, d, a) => {
                    let a = mask(f, &mut insts, &mut pos, a);
                    insts.push(Inst::AtomicLoad(m, o, d, a));
                    pos.push(p);
                }
                Inst::AtomicStore(m, o, a, s) => {
                    let a = mask(f, &mut insts, &mut pos, a);
                    insts.push(Inst::AtomicStore(m, o, a, s));
                    pos.push(p);
                }
                Inst::AtomicRmw(op, m, o, d, a, s) => {
                    let a = mask(f, &mut insts, &mut pos, a);
                    insts.push(Inst::AtomicRmw(op, m, o, d, a, s));
                    pos.push(p);
                }
                Inst::AtomicCas(m, o1, o2, d, a, e, nw) => {
                    let a = mask(f, &mut insts, &mut pos, a);
                    insts.push(Inst::AtomicCas(m, o1, o2, d, a, e, nw));
                    pos.push(p);
                }
                Inst::Copy(dst, src, len) => {
                    let dst = mask(f, &mut insts, &mut pos, dst);
                    let src = mask(f, &mut insts, &mut pos, src);
                    insts.push(Inst::Copy(dst, src, len));
                    pos.push(p);
                }
                Inst::Call(callee, args, rets) => {
                    let callee = match callee {
                        Callee::Indirect(v) => Callee::Indirect(mask(f, &mut insts, &mut pos, v)),
                        Callee::CIndirect(v, sig) => Callee::CIndirect(mask(f, &mut insts, &mut pos, v), sig),
                        other => other,
                    };
                    insts.push(Inst::Call(callee, args, rets));
                    pos.push(p);
                }
                other => {
                    insts.push(other);
                    pos.push(p);
                }
            }
            for d in defs {
                cache.remove(&d.0);
            }
        }
        // a return gives the shadow frame back
        if has_frame {
            if let Term::Ret(_) = b.term {
                insts.push(Inst::Store(Mem::Int(8, false), t_ss, 0, ssp0));
                pos.push(b.term_pos);
            }
        }
        out.push(Block { insts, pos, term: b.term, term_pos: b.term_pos, count: -1.0 });
    }
    out.extend(extra);
    f.blocks = out;
    f.slots.clear();
    // (the slots are gone into the shadow stack: no variable locations)
    f.dbg.clear();
    // facts about the old (offset) values do not hold for native addresses
    f.deref.clear();
    f.noalias.clear();
    f.withheld_noalias.clear();
    f.verify().map_err(|e| format!("{}: web32 transform made invalid RIR: {}", f.name, e))
}
