//! RIR -> WebAssembly module, size first.
//!
//! A [`Module`] is built from a CHOSEN set of RIR functions (the link step drives it from a
//! trace); every call to a function outside the set goes to one shared "untraced" stub. The
//! output is a complete module: deduplicated function types sorted by use, imports for the host
//! symbols the [`Link`] binds, one funcref table for function pointers, one linear memory (a
//! downward-growing shadow stack, then the data the link step placed, then the heap), active or
//! passive data segments, and an exception tag for unwinding.
//!
//! Per function:
//! - Structured control flow from the CFG by the dominator-tree method (Ramsey, "Beyond
//!   Relooper"): merge nodes become `block`s, loop headers `loop`s, other dominator children are
//!   placed inline at their one forward in-edge; switches become `br_table`. An irreducible CFG
//!   runs as a dispatch loop over a label local.
//! - Expression trees: a value defined once and used once by the next instruction that reads it
//!   stays on the operand stack (no reordering is ever done, so effects keep their order);
//!   compares feed `br_if`/`select` as i32 without the widening, and frame and constant-offset
//!   addresses fold into the memory instruction's offset.
//! - Locals: vregs of one wasm type that are never live at once share a local (greedy colouring
//!   over liveness, moves coalesced where possible); the most used locals get the 1-byte indices.
//! - Slots live in linear memory below the shadow stack pointer (global 0); traps are
//!   `unreachable`; `Term::Resume` throws the module's tag and `Term::Invoke` is a `try_table`.
//! Integers follow RIR: 64-bit registers holding narrow values re-extended after each op.

use crate::rir::*;
use std::collections::HashMap;

// ------------------------------------------------------------ encoding

pub fn uleb(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

pub fn sleb(out: &mut Vec<u8>, mut v: i64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0) {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// The bytes of `v` as an unsigned LEB128.
fn uleb_len(v: u64) -> usize {
    let mut out = Vec::new();
    uleb(&mut out, v);
    out.len()
}

/// The bytes of `v` as a signed LEB128.
fn sleb_len(v: i64) -> i64 {
    let mut out = Vec::new();
    sleb(&mut out, v);
    out.len() as i64
}

fn name(out: &mut Vec<u8>, s: &str) {
    uleb(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

fn section(out: &mut Vec<u8>, id: u8, body: &[u8]) {
    out.push(id);
    uleb(out, body.len() as u64);
    out.extend_from_slice(body);
}

/// wasm value types (the byte is the encoding)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Val {
    I32 = 0x7F,
    I64 = 0x7E,
    F32 = 0x7D,
    F64 = 0x7C,
    V128 = 0x7B,
}

fn val_of(c: Cls) -> Result<Val, String> {
    Ok(match c {
        Cls::I => Val::I64,
        Cls::F32 => Val::F32,
        Cls::F64 => Val::F64,
        Cls::V128 => Val::V128,
        Cls::V256 => return Err("256-bit vectors have no wasm form".into()),
    })
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct FnType {
    pub params: Vec<Val>,
    pub results: Vec<Val>,
}

/// The external C ABI has wasm value widths, independent of RIR's i64 integer
/// registers. Aggregates already use CSig's private-copy/sret pointer convention;
/// trivial scalar aggregates are classified as Scalar by the frontend.
fn boundary_type(sig: &CSig, internal: &FnType, memory64: bool) -> Result<FnType, String> {
    let pointer = if memory64 { Val::I64 } else { Val::I32 };
    let mut params = Vec::new();
    if sig.ret.is_some() { params.push(pointer); }
    for arg in &sig.args {
        params.push(match arg {
            CArg::Scalar(Mem::Int(n, _)) => if *n <= 4 { Val::I32 } else { Val::I64 },
            CArg::Scalar(Mem::F32) => Val::F32,
            CArg::Scalar(Mem::F64) => Val::F64,
            CArg::Agg(_) => pointer,
        });
    }
    if params.len() != internal.params.len() {
        return Err("C boundary parameter count differs from its RIR signature".into());
    }
    let mut results = internal.results.clone();
    if sig.ret_narrow.is_some() && results == [Val::I64] { results[0] = Val::I32; }
    Ok(FnType { params, results })
}

fn boundary_signed(sig: &CSig, index: usize) -> bool {
    index.checked_sub(usize::from(sig.ret.is_some()))
        .and_then(|i| sig.args.get(i))
        .is_some_and(|a| matches!(a, CArg::Scalar(Mem::Int(_, true))))
}

// ------------------------------------------------------------ link interface

/// How a host symbol (`Callee::Host` / `Callee::CHost`) is bound in the module.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HostBind {
    /// an imported function (module, field); its type is the first call site's
    Import(String, String),
    /// a routine the module carries itself
    Builtin(Builtin),
    /// the call traps (a path the export never takes)
    Trap,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Builtin {
    /// (memory_index) -> pages, using the module's address width.
    MemorySize,
    /// (memory_index, delta_pages) -> previous pages, or usize::MAX.
    MemoryGrow,
    /// (address, expected, timeout_ns) -> wait status. Unshared memory traps on equality.
    AtomicWait32,
    /// (address, count) -> number woken (zero for unshared memory).
    AtomicNotify,
    /// (dst, src, n) -> [dst]: memory.copy
    Memcpy,
    /// (dst, src, n) -> [dst]: memory.copy (overlap-safe)
    Memmove,
    /// (dst, byte, n) -> [dst]: memory.fill
    Memset,
    /// (f, data) -> u64: calls fn pointer `f(data)`; 0 when it returned, 1 when it unwound
    CatchPanic,
    /// does nothing; its results are zero (single-threaded waits and wakes)
    Nop,
    /// does nothing; its integer results are 1: the one thread's id (never 0, the "no owner"
    /// of locks), "is the main thread"
    One,
    /// thread-local keys of a single-threaded program: (dtor) -> key, (key) -> value,
    /// (key, value); a table in linear memory
    KeyCreate,
    KeyGet,
    KeySet,
}

/// What the link step tells the backend about the symbols code names.
pub trait Link {
    /// the value of symbol `id` as `Inst::Addr`, `Inst::TlsAddr` or `Inst::TlsStatic` use it (a linear-memory
    /// address or a plain number; the backend adds `Sym::off`)
    fn sym(&self, id: u32) -> u64;
    /// how a call to host symbol `id` is bound
    fn host(&self, id: u32) -> HostBind;
}

#[derive(Clone, Debug)]
pub struct Options {
    /// 64-bit linear memory (memory64: addresses are i64, no wraps); the wasm64 target. Default:
    /// 32-bit memory (wasm32)
    pub memory64: bool,
    /// A shared memory requires a maximum in pages. None is the single-thread page profile.
    pub shared_max_pages: Option<u64>,
    /// bytes of shadow stack (at the bottom of memory, growing down)
    pub stack_size: u32,
    /// pages (64 KiB) of heap after the data, in the initial memory size
    pub heap_pages: u32,
    /// untraced calls call the import `env.u(id: i32)` before trapping (for a message);
    /// false: they are a bare `unreachable`
    pub untraced_import: bool,
    /// emit a `name` section with function names
    pub names: bool,
    /// export the tag as "t" (for host code that throws into the module)
    pub export_tag: bool,
    /// exceptions as `try_table` (exnref); default: the legacy `try`/`catch_all` every engine
    /// runs (and 5 bytes smaller per invoke)
    pub exnref: bool,
    /// data at the bottom of memory (small addresses: short constants and offsets), the stack
    /// above it; false: the stack at the bottom (an overflow traps instead of reaching data)
    pub data_first: bool,
    /// `local.set x; local.get x` becomes `local.tee x`, or stays on the stack for a single use
    pub tee: bool,
    /// cheap single-definition constants are recomputed at each use instead of living in a local
    pub remat: bool,
    /// single-use values move down to their reader (`sink_trees`): pushed there, not via a local
    pub sink: bool,
    /// parameters of a declared narrow integer or pointer type (Func::param_ints) are i32
    /// whatever their callers pass (else only when every direct call passes a narrow value)
    pub narrow_declared: bool,
    /// the same for frame addresses used mostly as memory bases (the frame offset folds into
    /// the access)
    pub remat_slots: bool,
    /// a function with a frame and several returns keeps the caller's stack pointer in a local
    pub sp0: bool,
    /// panics abort: an invoke is a plain call, a resume traps, cleanup pads and the exception
    /// tag go away (rustc panic=abort)
    pub panic_abort: bool,
    /// function order: false = by use count (the most called get 1-byte indices); true = by
    /// name (a crate's and a generic's functions adjacent: similar code compresses together)
    pub order_by_name: bool,
    /// identical function bodies are emitted once
    pub fold_identical: bool,
    /// functions ordered by their body bytes (similar bodies adjacent for the compressor)
    pub order_by_bytes: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { memory64: false, shared_max_pages: None, stack_size: 1 << 20, heap_pages: 16, untraced_import: true, names: false, export_tag: true, exnref: false, data_first: false, tee: true, remat: true, sink: true, narrow_declared: true, remat_slots: true, sp0: false, panic_abort: false, order_by_name: true, fold_identical: true, order_by_bytes: true }
    }
}

/// Bytes per part of the output (for size work).
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub total: usize,
    pub types: usize,
    pub imports: usize,
    pub funcs: usize,
    pub code: usize,
    pub data: usize,
    pub elems: usize,
    pub names: usize,
    pub other: usize,
    pub n_funcs: usize,
    pub n_types: usize,
    pub n_imports: usize,
    /// functions whose body could not be lowered (id, why): they trap
    pub failed: Vec<(u32, String)>,
    pub untraced_sites: usize,
    /// (caller slot, target slot) for each untraced call encountered while lowering.
    pub untraced_calls: Vec<(u32, u32)>,
    /// function bodies folded into an identical one
    pub n_folded: usize,
    /// (slot id, function name, body bytes) of every chosen function
    pub fn_sizes: Vec<(u32, String, usize)>,
    /// MAKEPAD_RUN_WASM_EXPORT_FNLIST: (slot id, [(inline context, code bytes)]) of every chosen body
    pub fn_ctx_bytes: Vec<(u32, Vec<(u64, i64)>)>,
    /// (slot id, absolute module offset, encoded body length), including final
    /// relocation encodings. Folded aliases own no bytes: (slot, 0, 0).
    pub fn_ranges: Vec<(u32, usize, usize)>,
    /// Folded slots' shared implementation ranges. These bytes are already
    /// accounted for by the canonical body; aliases do not own another copy.
    pub fn_alias_ranges: Vec<(u32, usize, usize)>,
}

pub struct Output {
    pub bytes: Vec<u8>,
    pub stats: Stats,
    /// Actual function imports, including those requested by generated helpers.
    pub imports: Vec<(String, String)>,
}

// ------------------------------------------------------------ module

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Helper {
    FMinNum(bool),
    FMaxNum(bool),
    FMinLt(bool),
    FMaxGt(bool),
    RoundAway(bool),
    MulHi(bool),
    Bswap,
    Rev,
    Fma64,
    Builtin(Builtin, u32),
    Untraced,
}

/// A function reference resolved when the module is assembled.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum FRef {
    Import(u32),
    User(u32),
    Helper(u32),
}

#[derive(Clone, Copy, Debug)]
enum Fix {
    Func(FRef),
    Type(u32),
}

struct Body {
    bytes: Vec<u8>,
    fix: Vec<(u32, Fix)>,
    ty: u32,
    /// (inline context, code bytes) when ctx_bytes_on
    ctx: Vec<(u64, i64)>,
}

struct DataSeg {
    addr: Option<u64>,
    bytes: Vec<u8>,
}

struct Shared {
    types: Vec<FnType>,
    type_ix: HashMap<FnType, u32>,
    imports: Vec<(String, String, u32)>,
    import_ix: HashMap<(String, String), u32>,
    helpers: Vec<Helper>,
    helper_ix: HashMap<Helper, u32>,
    helper_bodies: Vec<Option<Body>>,
    table: Vec<Option<FRef>>,
    table_ix: HashMap<u32, u32>,
    untraced_entry: Option<u32>,
    untraced_targets: Vec<u32>,
    uses_tag: bool,
    uses_table: bool,
    /// the thread-local key table (Builtin::Key*)
    keys_base: u64,
}

impl Shared {
    fn ty(&mut self, t: FnType) -> u32 {
        if let Some(&i) = self.type_ix.get(&t) {
            return i;
        }
        let i = self.types.len() as u32;
        self.types.push(t.clone());
        self.type_ix.insert(t, i);
        i
    }
    fn import(&mut self, m: &str, f: &str, ty: u32) -> u32 {
        let k = (m.to_string(), f.to_string());
        if let Some(&i) = self.import_ix.get(&k) {
            return i;
        }
        let i = self.imports.len() as u32;
        self.imports.push((k.0.clone(), k.1.clone(), ty));
        self.import_ix.insert(k, i);
        i
    }
    fn helper(&mut self, h: Helper) -> u32 {
        if let Some(&i) = self.helper_ix.get(&h) {
            return i;
        }
        let i = self.helpers.len() as u32;
        self.helpers.push(h);
        self.helper_bodies.push(None);
        self.helper_ix.insert(h, i);
        i
    }
}

pub struct Module<'a> {
    opts: Options,
    funcs: Vec<(u32, &'a Func)>,
    fn_ix: HashMap<u32, u32>,
    exports: Vec<(String, u32)>,
    data: Vec<DataSeg>,
    data_end: u64,
    sh: Shared,
}

impl<'a> Module<'a> {
    pub fn new(opts: Options) -> Self {
        let stack = (opts.stack_size as u64 + 15) & !15;
        let data_end = if opts.data_first { 16 } else { stack.max(16) };
        Module {
            opts,
            funcs: Vec::new(),
            fn_ix: HashMap::new(),
            exports: Vec::new(),
            data: Vec::new(),
            data_end,
            sh: Shared {
                types: Vec::new(),
                type_ix: HashMap::new(),
                imports: Vec::new(),
                import_ix: HashMap::new(),
                helpers: Vec::new(),
                helper_ix: HashMap::new(),
                helper_bodies: Vec::new(),
                table: vec![None],
                table_ix: HashMap::new(),
                untraced_entry: None,
                untraced_targets: Vec::new(),
                uses_tag: false,
                uses_table: false,
                keys_base: 0,
            },
        }
    }

    /// Adds function slot `id` with body `f` to the chosen set.
    pub fn add_func(&mut self, id: u32, f: &'a Func) {
        if self.fn_ix.contains_key(&id) {
            return;
        }
        self.fn_ix.insert(id, self.funcs.len() as u32);
        self.funcs.push((id, f));
    }

    pub fn has_func(&self, id: u32) -> bool {
        self.fn_ix.contains_key(&id)
    }

    /// Exports function `id` (in the chosen set) as `name`.
    pub fn export_func(&mut self, id: u32, name: &str) {
        self.exports.push((name.to_string(), id));
    }

    /// The table index that is function `id`'s pointer value (`Inst::FnAddr`, and what the link
    /// step writes for a function pointer in data). A function outside the chosen set gets the
    /// one shared trapping entry. 0 is the null pointer. Call after the set is complete.
    pub fn table_index(&mut self, id: u32) -> u32 {
        table_index(&mut self.sh, &self.fn_ix, id)
    }

    /// Reserves `size` bytes (aligned) of zero-initialised linear memory in the data area.
    pub fn place(&mut self, size: u64, align: u64) -> u64 {
        let al = align.max(1);
        let at = (self.data_end + al - 1) / al * al;
        self.data_end = at + size;
        at
    }

    /// Initial bytes at linear-memory address `addr` (placed with `place`). Zero runs are not
    /// stored.
    pub fn data_at(&mut self, addr: u64, bytes: Vec<u8>) {
        self.data_end = self.data_end.max(addr + bytes.len() as u64);
        self.data.push(DataSeg { addr: Some(addr), bytes });
    }

    /// A passive segment (copied in with `memory.init` by code that wants it); its index.
    pub fn data_passive(&mut self, bytes: Vec<u8>) -> u32 {
        self.data.push(DataSeg { addr: None, bytes });
        self.data.len() as u32 - 1
    }

    /// End of the placed data (the heap starts at the next 16 bytes).
    pub fn data_end(&self) -> u64 {
        self.data_end
    }

    /// Every function's signature facts. A function the table, an export or a C ABI names keeps
    /// the plain one (indirect callers and hosts know only that); the others pass a reference
    /// parameter (an address in the 4 GB memory) and a parameter the body reads only the low
    /// half of as an i32, and return an i32 when every return value extends from one. Two
    /// rounds: a parameter's uses as a narrow argument of another function count from the second.
    fn signatures(&self, funcs: &[(u32, &Func)], body_of: &dyn Fn(usize) -> &'a Func, link: &dyn Link) -> Vec<Sig> {
        let n = funcs.len();
        let mut sigs: Vec<Sig> = funcs.iter().map(|(_, f)| Sig { params: vec![0; f.params.len()], ret: vec![0; f.rets.len()] }).collect();
        if self.opts.memory64 {
            return sigs;
        }
        let mut fixed = vec![false; n];
        for (&id, _) in &self.sh.table_ix {
            if let Some(&u) = self.fn_ix.get(&id) {
                fixed[u as usize] = true;
            }
        }
        for (_, id) in &self.exports {
            if let Some(&u) = self.fn_ix.get(id) {
                fixed[u as usize] = true;
            }
        }
        for u in 0..n {
            let f = body_of(u);
            if funcs[u].1.cabi.is_some() {
                fixed[u] = true;
            }
            for b in &f.blocks {
                for i in &b.insts {
                    if let Inst::FnAddr(_, t) = i {
                        if let Some(&c) = self.fn_ix.get(t) {
                            fixed[c as usize] = true;
                        }
                    }
                }
            }
        }
        // per function, per parameter: what every direct call passes (the meet of the arguments'
        // kinds over the module's call sites, from the previous round's kinds; UNSET: no call
        // seen). A parameter every caller passes zero- or sign-extended from its low 32 bits (a
        // usize, u32 or i32 of a wasm32 program: their values are always extended) is that kind
        // in the callee -- an i32 -- as rustc's wasm32 ABI passes them.
        const UNSET: u8 = 0xFF;
        let mut passed: Vec<Vec<u8>> = funcs.iter().map(|(_, f)| vec![UNSET; f.params.len()]).collect();
        for _round in 0..4 {
            let prev = sigs.clone();
            let csig = |id: u32| self.fn_ix.get(&id).map(|&u| &prev[u as usize]);
            let mut next: Vec<Vec<u8>> = funcs.iter().map(|(_, f)| vec![UNSET; f.params.len()]).collect();
            for u in 0..n {
                let f = body_of(u);
                let seeds: Vec<u8> = f.params.iter().enumerate().map(|(i, p)| {
                    if fixed[u] || f.vregs[p.0 as usize] != Cls::I {
                        0
                    } else if f.nonnull.contains(p) || f.deref.iter().any(|(d, _)| d == p) {
                        KZ
                    } else if let Some(&(_, _, signed)) = f.param_ints.iter().find(|x| x.0 == *p).filter(|x| x.1 <= 4 && self.opts.narrow_declared) {
                        // a declared integer or pointer of at most 32 bits (a wasm32 usize, u32,
                        // i32, bool, pointer): an i32 as rustc's wasm32 ABI passes it, whatever the
                        // callers computed it from (a wide value is wrapped at the call)
                        if signed { KS } else { KZ }
                    } else if passed[u][i] != UNSET && passed[u][i] & (KZ | KS) != 0 {
                        passed[u][i] & (KZ | KS)
                    } else {
                        KL
                    }
                }).collect();
                let k = narrow_kinds(f, &self.opts, link, &seeds, &csig);
                for b in &f.blocks {
                    for i in &b.insts {
                        if let Inst::Call(Callee::Fn(id), args, _) = i {
                            if let Some(&c) = self.fn_ix.get(id) {
                                let acc = &mut next[c as usize];
                                for (j, a) in args.iter().enumerate().take(acc.len()) {
                                    acc[j] &= k[a.0 as usize] & (KZ | KS);
                                }
                            }
                        }
                    }
                }
                if fixed[u] {
                    continue;
                }
                let params: Vec<u8> = f.params.iter().map(|p| k[p.0 as usize]).collect();
                let mut ret: Vec<u8> = f.rets.iter().map(|c| if *c == Cls::I { KZ | KS } else { 0 }).collect();
                for b in &f.blocks {
                    if let Term::Ret(vs) = &b.term {
                        for (r, v) in ret.iter_mut().zip(vs) {
                            *r &= k[v.0 as usize] & (KZ | KS);
                        }
                    }
                }
                sigs[u] = Sig { params, ret };
            }
            passed = next;
        }
        sigs
    }

    /// Lowers every function and assembles the module.
    pub fn finish(mut self, options: &relax_session::Options, link: &dyn Link) -> Output {
        let opts = self.opts.clone();
        // user function types first (their indices are needed by every call)
        let mut bodies: Vec<Body> = Vec::with_capacity(self.funcs.len());
        let mut failed = Vec::new();
        let mut untraced_calls = Vec::new();
        let funcs = std::mem::take(&mut self.funcs);
        let prepared: Vec<Option<Func>> = funcs.iter().map(|(_, f)| prepare(&opts, options, f, &self.fn_ix, link)).collect();
        let body_of = |u: usize| prepared[u].as_ref().unwrap_or(funcs[u].1);
        let sigs = self.signatures(&funcs, &body_of, link);
        for (u, (id, f)) in funcs.iter().enumerate() {
            let first = self.sh.untraced_targets.len();
            let r = lower_func(&opts, options, body_of(u), u, &self.fn_ix, &funcs, &sigs, link, &mut self.sh);
            untraced_calls.extend(self.sh.untraced_targets[first..].iter().map(|target| (*id, *target)));
            match r {
                Ok(b) => bodies.push(b),
                Err(e) => {
                    failed.push((*id, e));
                    let ty = func_type_sig(f, &sigs[u]).map(|t| self.sh.ty(t)).unwrap_or_else(|_| self.sh.ty(FnType { params: vec![], results: vec![] }));
                    bodies.push(Body { bytes: vec![0, 0x00, 0x0B], fix: vec![], ty, ctx: vec![] });
                }
            }
        }
        // Export wrappers alone adapt the ABI. The function table and all internal
        // direct/indirect calls continue to name the original RIR bodies.
        let mut export_ix = HashMap::new();
        for (_, id) in &self.exports {
            if export_ix.contains_key(id) { continue; }
            let u = self.fn_ix[id];
            let f = funcs[u as usize].1;
            let Some(sig) = &f.cabi else { export_ix.insert(*id, u); continue; };
            let external = func_type(f).and_then(|t| boundary_type(sig, &t, opts.memory64));
            match external {
                Ok(t) => {
                    let mut bytes = vec![0]; // no locals
                    for (i, p) in t.params.iter().enumerate() {
                        bytes.push(0x20);
                        uleb(&mut bytes, i as u64);
                        if *p == Val::I32 {
                            bytes.push(if boundary_signed(sig, i) { 0xAC } else { 0xAD });
                            if let Some(CArg::Scalar(Mem::Int(n, signed))) = i.checked_sub(usize::from(sig.ret.is_some())).and_then(|i| sig.args.get(i)) {
                                if *n < 4 {
                                    if *signed {
                                        bytes.push(if *n == 1 { 0xC2 } else { 0xC3 });
                                    } else {
                                        bytes.push(0x42);
                                        sleb(&mut bytes, (1i64 << (*n * 8)) - 1);
                                        bytes.push(0x83);
                                    }
                                }
                            }
                        }
                    }
                    bytes.push(0x10);
                    let fix = vec![(bytes.len() as u32, Fix::Func(FRef::User(u)))];
                    if t.results == [Val::I32] { bytes.push(0xA7); }
                    bytes.push(0x0B);
                    export_ix.insert(*id, bodies.len() as u32);
                    bodies.push(Body { bytes, fix, ty: self.sh.ty(t), ctx: vec![] });
                }
                Err(e) => { failed.push((*id, e)); export_ix.insert(*id, u); }
            }
        }
        // helpers (bodies made on demand; a helper may ask for another)
        if self.sh.helpers.iter().any(|h| matches!(h, Helper::Builtin(Builtin::KeyCreate | Builtin::KeyGet | Builtin::KeySet, _))) {
            self.sh.keys_base = self.place(8 * 256, 8);
        }
        let mut k = 0;
        while k < self.sh.helpers.len() {
            if self.sh.helper_bodies[k].is_none() {
                let h = self.sh.helpers[k];
                let b = helper_body(&opts, h, &mut self.sh, &self.fn_ix);
                self.sh.helper_bodies[k] = Some(b);
            }
            k += 1;
        }
        let tag_ty = if self.sh.uses_tag { Some(self.sh.ty(FnType { params: vec![], results: vec![] })) } else { None };

        // ---- final orders: types and functions by use count (1-byte indices for the most used)
        let n_user = bodies.len();
        let helper_bodies: Vec<Body> = self.sh.helper_bodies.drain(..).map(|b| b.unwrap()).collect();
        let n_help = helper_bodies.len();
        let ni = self.sh.imports.len();
        let mut fuse = vec![0u64; ni + n_user + n_help];
        let mut tuse = vec![0u64; self.sh.types.len()];
        let gidx0 = |r: FRef| -> usize {
            match r {
                FRef::Import(i) => i as usize,
                FRef::User(i) => ni + i as usize,
                FRef::Helper(i) => ni + n_user + i as usize,
            }
        };
        // identical functions (type, code and call targets, the targets compared after folding)
        // become one: monomorphic copies of generic code that lower to the same wasm
        let mut canon: Vec<usize> = (0..ni + n_user + n_help).collect();
        if opts.fold_identical {
            let all_b: Vec<&Body> = bodies.iter().chain(helper_bodies.iter()).collect();
            loop {
                let mut seen: HashMap<(u32, &[u8], Vec<(u32, bool, u64)>), usize> = HashMap::new();
                let mut changed = false;
                for (k, b) in all_b.iter().enumerate() {
                    let g = ni + k;
                    if canon[g] != g {
                        continue;
                    }
                    let fx: Vec<(u32, bool, u64)> = b.fix.iter().map(|(p, f)| match f {
                        Fix::Func(r) => (*p, false, canon[gidx0(*r)] as u64),
                        Fix::Type(t) => (*p, true, *t as u64),
                    }).collect();
                    match seen.entry((b.ty, &b.bytes[..], fx)) {
                        std::collections::hash_map::Entry::Occupied(e) => {
                            canon[g] = *e.get();
                            changed = true;
                        }
                        std::collections::hash_map::Entry::Vacant(e) => {
                            e.insert(g);
                        }
                    }
                }
                if !changed {
                    break;
                }
            }
            // chains: a merged function's canonical one may itself have merged later
            for g in 0..canon.len() {
                let mut c = canon[g];
                while canon[c] != c {
                    c = canon[c];
                }
                canon[g] = c;
            }
        }
        let gidx = |r: FRef| -> usize { canon[gidx0(r)] };
        for (k, b) in bodies.iter().chain(helper_bodies.iter()).enumerate() {
            if canon[ni + k] != ni + k {
                continue;
            }
            tuse[b.ty as usize] += 1;
            for (_, fx) in &b.fix {
                match fx {
                    Fix::Func(r) => fuse[gidx(*r)] += 1,
                    Fix::Type(t) => tuse[*t as usize] += 1,
                }
            }
        }
        for (_, _, t) in &self.sh.imports {
            tuse[*t as usize] += 1;
        }
        if let Some(t) = tag_ty {
            tuse[t as usize] += 1;
        }
        // types: by use, ties by first appearance
        let mut tord: Vec<u32> = (0..self.sh.types.len() as u32).collect();
        tord.sort_by(|a, b| tuse[*b as usize].cmp(&tuse[*a as usize]).then(a.cmp(b)));
        let mut tmap = vec![0u32; tord.len()];
        for (n, t) in tord.iter().enumerate() {
            tmap[*t as usize] = n as u32;
        }
        // imports: by use; defined functions: by use (exports and table entries count too)
        for (_, id) in &self.exports {
            if let Some(&u) = export_ix.get(id) {
                fuse[canon[ni + u as usize]] += 1;
            }
        }
        for e in self.sh.table.iter().flatten() {
            fuse[gidx(*e)] += 1;
        }
        let mut iord: Vec<usize> = (0..ni).collect();
        iord.sort_by(|a, b| fuse[*b].cmp(&fuse[*a]).then(a.cmp(b)));
        let mut dord: Vec<usize> = (ni..ni + n_user + n_help).filter(|g| canon[*g] == *g).collect();
        if opts.order_by_bytes {
            // similar bodies next to each other (the compressor's window): by their bytes
            let body = |g: usize| -> &[u8] { if g < ni + n_user { &bodies[g - ni].bytes } else { &helper_bodies[g - ni - n_user].bytes } };
            dord.sort_by(|a, b| body(*a).cmp(body(*b)).then(a.cmp(b)));
        } else if opts.order_by_name {
            let key = |g: usize| if g < ni + funcs.len() { funcs[g - ni].1.name.as_str() } else { "\u{10FFFF}" };
            dord.sort_by(|a, b| key(*a).cmp(key(*b)).then(a.cmp(b)));
        } else {
            dord.sort_by(|a, b| fuse[*b].cmp(&fuse[*a]).then(a.cmp(b)));
        }
        let mut fmap = vec![0u32; ni + n_user + n_help];
        for (n, g) in iord.iter().chain(dord.iter()).enumerate() {
            fmap[*g] = n as u32;
        }
        for g in 0..fmap.len() {
            fmap[g] = fmap[canon[g]];
        }

        let mut out = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
        let mut st = Stats::default();
        let sz = |st: &mut Stats, out: &Vec<u8>, before: usize, which: u8| {
            let n = out.len() - before;
            match which {
                1 => st.types += n,
                2 => st.imports += n,
                3 => st.funcs += n,
                10 => st.code += n,
                11 => st.data += n,
                9 => st.elems += n,
                0 => st.names += n,
                _ => st.other += n,
            }
        };
        // type section
        let mut s = Vec::new();
        uleb(&mut s, tord.len() as u64);
        for t in &tord {
            let t = &self.sh.types[*t as usize];
            s.push(0x60);
            uleb(&mut s, t.params.len() as u64);
            s.extend(t.params.iter().map(|v| *v as u8));
            uleb(&mut s, t.results.len() as u64);
            s.extend(t.results.iter().map(|v| *v as u8));
        }
        let b0 = out.len();
        section(&mut out, 1, &s);
        sz(&mut st, &out, b0, 1);
        // imports
        if ni > 0 {
            let mut s = Vec::new();
            uleb(&mut s, ni as u64);
            for i in &iord {
                let (m, f, t) = &self.sh.imports[*i];
                name(&mut s, m);
                name(&mut s, f);
                s.push(0);
                uleb(&mut s, tmap[*t as usize] as u64);
            }
            let b0 = out.len();
            section(&mut out, 2, &s);
            sz(&mut st, &out, b0, 2);
        }
        // function section
        let all: Vec<&Body> = bodies.iter().chain(helper_bodies.iter()).collect();
        let mut s = Vec::new();
        uleb(&mut s, dord.len() as u64);
        for g in &dord {
            uleb(&mut s, tmap[all[*g - ni].ty as usize] as u64);
        }
        let b0 = out.len();
        section(&mut out, 3, &s);
        sz(&mut st, &out, b0, 3);
        // table
        let tlen = self.sh.table.len() as u64;
        if tlen > 1 || self.sh.uses_table {
            let mut s = vec![1, 0x70, 0];
            uleb(&mut s, tlen);
            let b0 = out.len();
            section(&mut out, 4, &s);
            sz(&mut st, &out, b0, 4);
        }
        // memory: [16, data_end) data, then the stack (growing down from stack_top), then the heap
        // (data_first); else the stack from 0, then data, then heap
        let stack_sz = (opts.stack_size as u64 + 15) & !15;
        let (stack_top, heap_base) = if opts.data_first {
            let t = ((self.data_end + 15) & !15) + stack_sz;
            (t, t)
        } else {
            (stack_sz, (self.data_end + 15) & !15)
        };
        let pages = (heap_base + 0xFFFF) / 0x10000 + opts.heap_pages as u64;
        // limits flags: 0x04 = 64-bit addresses
        let mut s = vec![1, (if opts.memory64 { 0x04 } else { 0x00 }) | if opts.shared_max_pages.is_some() { 3 } else { 0 }];
        uleb(&mut s, pages);
        if let Some(max) = opts.shared_max_pages {
            uleb(&mut s, max);
        }
        let b0 = out.len();
        section(&mut out, 5, &s);
        sz(&mut st, &out, b0, 5);
        // tag
        if let Some(t) = tag_ty {
            let mut s = vec![1, 0];
            uleb(&mut s, tmap[t as usize] as u64);
            let b0 = out.len();
            section(&mut out, 13, &s);
            sz(&mut st, &out, b0, 13);
        }
        // globals (the address type): 0 = shadow stack pointer, 1 = heap base (immutable)
        let mut s = vec![2];
        let (aty, acst) = if opts.memory64 { (0x7E, 0x42) } else { (0x7F, 0x41) };
        s.extend_from_slice(&[aty, 1, acst]);
        sleb(&mut s, stack_top as i64);
        s.push(0x0B);
        s.extend_from_slice(&[aty, 0, acst]);
        sleb(&mut s, heap_base as i64);
        s.push(0x0B);
        let b0 = out.len();
        section(&mut out, 6, &s);
        sz(&mut st, &out, b0, 6);
        // exports: memory "memory" (the name the web runtime and rustc's wasm modules use), heap
        // base "h", tag "t", then the functions
        let mut s = Vec::new();
        let mut n = 2u64 + self.exports.len() as u64;
        if tlen > 1 || self.sh.uses_table { n += 1; }
        if tag_ty.is_some() && opts.export_tag {
            n += 1;
        }
        uleb(&mut s, n);
        name(&mut s, "memory");
        s.extend_from_slice(&[2, 0]);
        name(&mut s, "h");
        s.extend_from_slice(&[3, 1]);
        if tlen > 1 || self.sh.uses_table {
            name(&mut s, "__indirect_function_table");
            s.extend_from_slice(&[1, 0]);
        }
        if tag_ty.is_some() && opts.export_tag {
            name(&mut s, "t");
            s.extend_from_slice(&[4, 0]);
        }
        for (nm, id) in &self.exports {
            name(&mut s, nm);
            s.push(0);
            let u = export_ix[id];
            uleb(&mut s, fmap[ni + u as usize] as u64);
        }
        let b0 = out.len();
        section(&mut out, 7, &s);
        sz(&mut st, &out, b0, 7);
        // element segment: the table from index 1
        if tlen > 1 {
            let mut s = vec![1, 0, 0x41, 1, 0x0B];
            uleb(&mut s, tlen - 1);
            for e in &self.sh.table[1..] {
                uleb(&mut s, fmap[gidx(e.unwrap())] as u64);
            }
            let b0 = out.len();
            section(&mut out, 9, &s);
            sz(&mut st, &out, b0, 9);
        }
        // data segments: compacted (zero runs dropped, near neighbours merged)
        let segs = compact_data(std::mem::take(&mut self.data));
        let passive = segs.iter().any(|d| d.addr.is_none());
        if passive {
            let mut s = Vec::new();
            uleb(&mut s, segs.len() as u64);
            let b0 = out.len();
            section(&mut out, 12, &s);
            sz(&mut st, &out, b0, 12);
        }
        // code
        let mut s = Vec::new();
        uleb(&mut s, dord.len() as u64);
        let mut fb = Vec::new();
        let mut body_ranges = vec![(0, 0); ni + all.len()];
        for g in &dord {
            let b = all[*g - ni];
            fb.clear();
            let mut at = 0usize;
            for (pos, fx) in &b.fix {
                fb.extend_from_slice(&b.bytes[at..*pos as usize]);
                match fx {
                    Fix::Func(r) => uleb(&mut fb, fmap[gidx(*r)] as u64),
                    Fix::Type(t) => uleb(&mut fb, tmap[*t as usize] as u64),
                }
                at = *pos as usize;
            }
            fb.extend_from_slice(&b.bytes[at..]);
            uleb(&mut s, fb.len() as u64);
            body_ranges[*g] = (s.len(), fb.len());
            s.extend_from_slice(&fb);
        }
        let b0 = out.len();
        section(&mut out, 10, &s);
        let code_start = out.len() - s.len();
        st.fn_ranges = funcs.iter().enumerate().map(|(k, (id, _))| {
            if canon[ni + k] != ni + k { return (*id, 0, 0); }
            let (offset, len) = body_ranges[ni + k];
            (*id, code_start + offset, len)
        }).collect();
        st.fn_alias_ranges = funcs.iter().enumerate().filter_map(|(k, (id, _))| {
            let canonical = canon[ni + k];
            if canonical == ni + k { return None; }
            let (offset, len) = body_ranges[canonical];
            Some((*id, code_start + offset, len))
        }).collect();
        sz(&mut st, &out, b0, 10);
        if !segs.is_empty() {
            let mut s = Vec::new();
            uleb(&mut s, segs.len() as u64);
            for d in &segs {
                match d.addr {
                    Some(a) => {
                        s.push(0);
                        if opts.memory64 {
                            s.push(0x42);
                            sleb(&mut s, a as i64);
                        } else {
                            s.push(0x41);
                            sleb(&mut s, a as u32 as i32 as i64);
                        }
                        s.push(0x0B);
                    }
                    None => s.push(1),
                }
                uleb(&mut s, d.bytes.len() as u64);
                s.extend_from_slice(&d.bytes);
            }
            let b0 = out.len();
            section(&mut out, 11, &s);
            sz(&mut st, &out, b0, 11);
        }
        if opts.names {
            let mut sub = Vec::new();
            uleb(&mut sub, funcs.iter().enumerate().filter(|(u, _)| canon[ni + u] == ni + u).count() as u64);
            let mut named: Vec<(u32, &str)> = funcs.iter().enumerate().filter(|(u, _)| canon[ni + u] == ni + u).map(|(u, (_, f))| (fmap[ni + u], f.name.as_str())).collect();
            named.sort();
            for (i, nm) in named {
                uleb(&mut sub, i as u64);
                name(&mut sub, nm);
            }
            let mut s = Vec::new();
            name(&mut s, "name");
            s.push(1);
            uleb(&mut s, sub.len() as u64);
            s.extend_from_slice(&sub);
            let b0 = out.len();
            section(&mut out, 0, &s);
            sz(&mut st, &out, b0, 0);
        }
        st.total = out.len();
        st.n_funcs = dord.len();
        st.n_folded = (ni..ni + n_user + n_help).filter(|g| canon[*g] != *g).count();
        st.n_types = tord.len();
        st.n_imports = ni;
        st.failed = failed;
        st.untraced_sites = untraced_calls.len();
        st.untraced_calls = untraced_calls;
        // a folded copy emits no body: 0 bytes
        st.fn_sizes = funcs.iter().zip(bodies.iter()).enumerate().map(|(k, ((id, f), b))| (*id, f.name.clone(), if canon[ni + k] == ni + k { b.bytes.len() } else { 0 })).collect();
        if ctx_bytes_on() {
            st.fn_ctx_bytes = funcs.iter().zip(bodies.iter_mut()).enumerate().filter(|(k, _)| canon[ni + k] == ni + k).map(|(_, ((id, _), b))| (*id, std::mem::take(&mut b.ctx))).collect();
        }
        let imports = self.sh.imports.iter().map(|(module, name, _)| (module.clone(), name.clone())).collect();
        Output { bytes: out, stats: st, imports }
    }
}

fn table_index(sh: &mut Shared, fn_ix: &HashMap<u32, u32>, id: u32) -> u32 {
    if let Some(&t) = sh.table_ix.get(&id) {
        return t;
    }
    let t = match fn_ix.get(&id) {
        Some(&u) => {
            sh.table.push(Some(FRef::User(u)));
            sh.table.len() as u32 - 1
        }
        None => match sh.untraced_entry {
            Some(t) => t,
            None => {
                let h = sh.helper(Helper::Untraced);
                sh.table.push(Some(FRef::Helper(h)));
                let t = sh.table.len() as u32 - 1;
                sh.untraced_entry = Some(t);
                t
            }
        },
    };
    sh.table_ix.insert(id, t);
    t
}

/// Data segments in address order with zero runs of 8+ bytes cut out and segments closer than
/// 8 bytes merged (a segment header costs about that much).
fn compact_data(segs: Vec<DataSeg>) -> Vec<DataSeg> {
    let mut active: Vec<DataSeg> = Vec::new();
    let mut passive = Vec::new();
    for d in segs {
        if d.addr.is_none() {
            passive.push(d);
            continue;
        }
        let a = d.addr.unwrap();
        // split at zero runs
        let b = &d.bytes;
        let mut i = 0;
        while i < b.len() {
            while i < b.len() && b[i] == 0 {
                i += 1;
            }
            if i >= b.len() {
                break;
            }
            let start = i;
            let mut end = i;
            let mut zeros = 0;
            while i < b.len() {
                if b[i] == 0 {
                    zeros += 1;
                    if zeros >= 8 {
                        break;
                    }
                } else {
                    zeros = 0;
                    end = i + 1;
                }
                i += 1;
            }
            active.push(DataSeg { addr: Some(a + start as u64), bytes: b[start..end].to_vec() });
        }
    }
    active.sort_by_key(|d| d.addr.unwrap());
    let mut merged: Vec<DataSeg> = Vec::new();
    for d in active {
        if let Some(last) = merged.last_mut() {
            let le = last.addr.unwrap() + last.bytes.len() as u64;
            let da = d.addr.unwrap();
            if da >= le && da - le < 8 {
                last.bytes.resize((da - last.addr.unwrap()) as usize, 0);
                last.bytes.extend_from_slice(&d.bytes);
                continue;
            }
            if da < le {
                // overlap: later bytes win
                let off = (da - last.addr.unwrap()) as usize;
                let need = off + d.bytes.len();
                if last.bytes.len() < need {
                    last.bytes.resize(need, 0);
                }
                last.bytes[off..need].copy_from_slice(&d.bytes);
                continue;
            }
        }
        merged.push(d);
    }
    merged.extend(passive);
    merged
}

fn func_type(f: &Func) -> Result<FnType, String> {
    let mut params = Vec::with_capacity(f.params.len());
    for p in &f.params {
        params.push(val_of(f.vregs[p.0 as usize])?);
    }
    let mut results = Vec::with_capacity(f.rets.len());
    for r in &f.rets {
        results.push(val_of(*r)?);
    }
    Ok(FnType { params, results })
}

// ------------------------------------------------------------ opcodes

const BLOCK: u8 = 0x02;
const LOOP: u8 = 0x03;
const IF: u8 = 0x04;
const ELSE: u8 = 0x05;
const THROW: u8 = 0x08;
const END: u8 = 0x0B;
const BR: u8 = 0x0C;
const BR_IF: u8 = 0x0D;
const BR_TABLE: u8 = 0x0E;
const RETURN: u8 = 0x0F;
const CALL: u8 = 0x10;
const CALL_INDIRECT: u8 = 0x11;
const DROP: u8 = 0x1A;
const SELECT: u8 = 0x1B;
const TRY_TABLE: u8 = 0x1F;
const TRY: u8 = 0x06;
const CATCH_ALL: u8 = 0x19;
const LGET: u8 = 0x20;
const LSET: u8 = 0x21;
const LTEE: u8 = 0x22;
const GGET: u8 = 0x23;
const GSET: u8 = 0x24;
const I32C: u8 = 0x41;
const I64C: u8 = 0x42;
const F32C: u8 = 0x43;
const F64C: u8 = 0x44;
const I32_EQZ: u8 = 0x45;
const I64_EQZ: u8 = 0x50;
const I32_ADD: u8 = 0x6A;
const I32_SUB: u8 = 0x6B;
const I64_AND: u8 = 0x83;
const I64_ADD: u8 = 0x7C;
const I64_SUB: u8 = 0x7D;
const I64_MUL: u8 = 0x7E;
const I64_OR: u8 = 0x84;
const I64_XOR: u8 = 0x85;
const I64_SHL: u8 = 0x86;
const I64_SHR_S: u8 = 0x87;
const I64_SHR_U: u8 = 0x88;
const WRAP: u8 = 0xA7;
const EXT_S: u8 = 0xAC;
const EXT_U: u8 = 0xAD;
const F64_PROMOTE: u8 = 0xBB;
const F32_DEMOTE: u8 = 0xB6;

// ------------------------------------------------------------ CFG analysis

struct Cfg {
    rpo: Vec<u32>,
    ix: Vec<u32>,
    kids: Vec<Vec<u32>>,
    loop_hdr: Vec<bool>,
    merge: Vec<bool>,
    reducible: bool,
}

fn analyse(f: &Func) -> Cfg {
    let n = f.blocks.len();
    let mut seen = vec![false; n];
    let mut post = Vec::with_capacity(n);
    let mut stack: Vec<(u32, Vec<u32>, usize)> = Vec::new();
    seen[0] = true;
    stack.push((0, f.blocks[0].term.succs().collect(), 0));
    while let Some(top) = stack.last_mut() {
        if top.2 < top.1.len() {
            let s = top.1[top.2];
            top.2 += 1;
            if !seen[s as usize] {
                seen[s as usize] = true;
                let ss = f.blocks[s as usize].term.succs().collect();
                stack.push((s, ss, 0));
            }
        } else {
            post.push(top.0);
            stack.pop();
        }
    }
    post.reverse();
    let rpo = post;
    let mut ix = vec![u32::MAX; n];
    for (i, b) in rpo.iter().enumerate() {
        ix[*b as usize] = i as u32;
    }
    let mut preds: Vec<Vec<u32>> = vec![Vec::new(); n];
    for &b in &rpo {
        for s in f.blocks[b as usize].term.succs() {
            preds[s as usize].push(b);
        }
    }
    // dominators (Cooper/Harvey/Kennedy)
    let mut idom = vec![u32::MAX; n];
    idom[0] = 0;
    let mut changed = true;
    while changed {
        changed = false;
        for &b in rpo.iter().skip(1) {
            let mut new = u32::MAX;
            for &p in &preds[b as usize] {
                if idom[p as usize] == u32::MAX {
                    continue;
                }
                if new == u32::MAX {
                    new = p;
                } else {
                    let (mut x, mut y) = (p, new);
                    while x != y {
                        while ix[x as usize] > ix[y as usize] {
                            x = idom[x as usize];
                        }
                        while ix[y as usize] > ix[x as usize] {
                            y = idom[y as usize];
                        }
                    }
                    new = x;
                }
            }
            if idom[b as usize] != new {
                idom[b as usize] = new;
                changed = true;
            }
        }
    }
    let mut kids = vec![Vec::new(); n];
    for &b in rpo.iter().skip(1) {
        kids[idom[b as usize] as usize].push(b);
    }
    let dominates = |a: u32, mut b: u32| -> bool {
        loop {
            if a == b {
                return true;
            }
            if b == 0 {
                return false;
            }
            b = idom[b as usize];
        }
    };
    let mut loop_hdr = vec![false; n];
    let mut merge = vec![false; n];
    let mut reducible = true;
    for &b in &rpo {
        let mut fwd = 0;
        for &p in &preds[b as usize] {
            if ix[p as usize] >= ix[b as usize] {
                loop_hdr[b as usize] = true;
                if !dominates(b, p) {
                    reducible = false;
                }
            } else {
                fwd += 1;
            }
        }
        merge[b as usize] = fwd >= 2;
    }
    Cfg { rpo, ix, kids, loop_hdr, merge, reducible }
}

// ------------------------------------------------------------ per-function lowering

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ctx {
    If,
    Loop(u32),
    Block(u32),
    Plain,
    /// the function's shared exit (Fx::exit): returns branch here, its end restores the stack
    /// pointer once
    Exit,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Want {
    Nat,
    /// i32, nonzero iff the value is nonzero
    Truthy,
    /// i32, nonzero iff the value is zero
    Inverted,
    /// the address type, with this much added (the result is the memarg offset)
    Addr(i64),
    /// i32: the value's low 32 bits (wasm32 narrow values: `Fx::nk`)
    Low,
}

/// narrow kinds of an integer vreg (`narrow_kinds`): its 64-bit value is the zero- (KZ) or
/// sign-extension (KS) of its low 32 bits, so it lives in an i32 local
const KZ: u8 = 1;
const KS: u8 = 2;
/// narrow by demand: every use reads only the low 32 bits (addresses, narrow stores, the
/// operands of narrow add / sub / mul / bitwise ops); the upper bits are unknown
const KL: u8 = 4;

fn const_kind(v: i64) -> u8 {
    (if (0..1i64 << 32).contains(&v) { KZ } else { 0 }) | (if (-(1i64 << 31)..1i64 << 31).contains(&v) { KS } else { 0 })
}

fn ty_kind(it: IntTy) -> u8 {
    match (it.bits, it.signed) {
        (b, false) if b < 32 => KZ | KS,
        (32, false) => KZ,
        (b, true) if b <= 32 => KS,
        _ => 0,
    }
}

/// Per vreg: the narrow kind of every definition's value (0: none, a wide i64). wasm32 only.
/// Copies, selects and bitwise ops take their operands' kinds (optimistic over cycles: a loop
/// variable enters its cycle from a definition outside it).
/// `pk`: per parameter its signature kind (0 wide; KZ; KL: a candidate the body's uses decide);
/// `csig`: a direct callee's signature (its narrow parameters read only the low half of an
/// argument; its narrow result defines a narrow value).
fn narrow_kinds<'s>(f: &Func, o: &Options, link: &dyn Link, pk: &[u8], csig: &dyn Fn(u32) -> Option<&'s Sig>) -> Vec<u8> {
    let nv = f.vregs.len();
    if o.memory64 {
        return vec![0; nv];
    }
    const UNSET: u8 = 0xFF;
    let mut k = vec![UNSET; nv];
    let pkind = |i: usize| pk.get(i).copied().unwrap_or(0) & (KZ | KS);
    for (i, p) in f.params.iter().enumerate() {
        k[p.0 as usize] = pkind(i);
    }
    let top = |k: &[u8], v: VReg| if k[v.0 as usize] == UNSET { KZ | KS } else { k[v.0 as usize] };
    for _ in 0..32 {
        let mut n = vec![UNSET; nv];
        for (i, p) in f.params.iter().enumerate() {
            n[p.0 as usize] = pkind(i);
        }
        let put = |n: &mut Vec<u8>, d: VReg, x: u8| {
            let d = d.0 as usize;
            n[d] = if n[d] == UNSET { x } else { n[d] & x };
        };
        for b in &f.blocks {
            for i in &b.insts {
                let load = |m: Mem| match m {
                    Mem::Int(1 | 2, false) => KZ | KS,
                    Mem::Int(1 | 2, true) | Mem::Int(4, true) => KS,
                    Mem::Int(4, false) => KZ,
                    _ => 0,
                };
                match *i {
                    Inst::Iconst(d, v) => put(&mut n, d, const_kind(v)),
                    Inst::Load(m, d, ..) | Inst::LoadX(m, d, ..) | Inst::AtomicLoad(m, _, d, _) => put(&mut n, d, load(m)),
                    Inst::IBin(op @ (IOp::And | IOp::Or | IOp::Xor), _, d, a, x) => {
                        let (ka, kx) = (top(&k, a), top(&k, x));
                        put(&mut n, d, if op == IOp::And { (ka | kx) & KZ | ka & kx & KS } else { ka & kx })
                    }
                    Inst::IBinI(op @ (IOp::And | IOp::Or | IOp::Xor), _, d, a, imm) => {
                        let (ka, kx) = (top(&k, a), const_kind(imm));
                        put(&mut n, d, if op == IOp::And { (ka | kx) & KZ | ka & kx & KS } else { ka & kx })
                    }
                    Inst::IBin(op, it, d, ..) | Inst::IBinI(op, it, d, ..) => put(&mut n, d, match op {
                        IOp::MulHi | IOp::Bzhi => 0,
                        _ => ty_kind(it),
                    }),
                    Inst::INeg(it, d, _) | Inst::INot(it, d, _) => put(&mut n, d, ty_kind(it)),
                    Inst::ICmp(_, _, d, ..) | Inst::ICmpI(_, _, d, ..) | Inst::FCmp(_, _, d, ..) => put(&mut n, d, KZ | KS),
                    Inst::Conv(Conv::IntToInt(it), d, x) => put(&mut n, d, if it.bits >= 64 { top(&k, x) } else { ty_kind(it) }),
                    Inst::Conv(Conv::FToInt(_, it), d, _) => put(&mut n, d, ty_kind(it)),
                    Inst::Conv(Conv::FToBits(false), d, _) => put(&mut n, d, KZ),
                    Inst::Conv(_, d, _) => put(&mut n, d, 0),
                    Inst::SlotAddr(d, _) => put(&mut n, d, KZ),
                    Inst::Addr(d, s) | Inst::TlsAddr(d, s) | Inst::TlsStatic(d, s) => put(&mut n, d, const_kind(link.sym(s.id).wrapping_add(s.off as u64) as i64) & KZ),
                    Inst::FnAddr(d, _) => put(&mut n, d, KZ),
                    Inst::Sel(d, _, a, x) if f.vregs[d.0 as usize] == Cls::I => put(&mut n, d, top(&k, a) & top(&k, x)),
                    Inst::Mov(d, a) if f.vregs[d.0 as usize] == Cls::I => put(&mut n, d, top(&k, a)),
                    Inst::VExtract(d, _, _) if f.vregs[d.0 as usize] == Cls::I => put(&mut n, d, KS),
                    Inst::Call(Callee::Fn(id) | Callee::CFn(id, _), _, ref rets) if rets.len() > 1 => {
                        for (k, r) in rets.iter().enumerate() {
                            put(&mut n, *r, csig(id).map_or(0, |s| s.ret.get(k).copied().unwrap_or(0)));
                        }
                    }
                    Inst::Call(Callee::Fn(id) | Callee::CFn(id, _), _, ref rets) if rets.len() == 1 => {
                        put(&mut n, rets[0], csig(id).map_or(0, |s| s.ret[0]))
                    }
                    _ => i.defs(|d| put(&mut n, d, 0)),
                }
            }
        }
        if n == k {
            break;
        }
        k = n;
    }
    for v in 0..nv {
        if k[v] == UNSET || f.vregs[v] != Cls::I {
            k[v] = 0;
        }
    }
    // narrow by demand (KL): candidates are the other integer vregs every definition of which is
    // an expression (not a call, not a parameter); a use that needs more than the low half removes
    // one, to a fixed point (whether an operand use is narrow depends on its result's form)
    let mut defs_ok = vec![true; nv];
    let mut has_def = vec![false; nv];
    for (i, p) in f.params.iter().enumerate() {
        // a parameter: a candidate only when its signature may be narrowed (KL)
        defs_ok[p.0 as usize] = pk.get(i).is_some_and(|x| *x == KL);
        has_def[p.0 as usize] = true;
    }
    for b in &f.blocks {
        for i in &b.insts {
            let expr = tree_def(i).is_some() && !matches!(i, Inst::Call(..));
            i.defs(|d| {
                has_def[d.0 as usize] = true;
                if !expr || f.params.contains(&d) {
                    defs_ok[d.0 as usize] = false;
                }
            });
        }
    }
    for v in 0..nv {
        if k[v] == 0 && f.vregs[v] == Cls::I && defs_ok[v] && has_def[v] {
            k[v] = KL;
        }
    }
    let mut uses: Vec<VReg> = Vec::new();
    let mut low: Vec<VReg> = Vec::new();
    loop {
        let mut changed = false;
        for b in &f.blocks {
            for i in &b.insts {
                uses.clear();
                let mut ds: Vec<VReg> = Vec::new();
                i.defs(|d| ds.push(d));
                let mut x = i.clone();
                x.vregs_mut(|v| uses.push(*v));
                for d in &ds {
                    if let Some(p) = uses.iter().position(|v| v == d) {
                        uses.remove(p);
                    }
                }
                if let Inst::VInsert(_, v, _, _) = i {
                    if !uses.contains(v) {
                        uses.push(*v);
                    }
                }
                low.clear();
                low_uses(i, &k, &mut low);
                // the arguments of a callee's narrow parameters
                if let Inst::Call(Callee::Fn(id) | Callee::CFn(id, _), args, _) = i {
                    if let Some(s) = csig(*id) {
                        for (a, p) in args.iter().zip(&s.params) {
                            if *p != 0 {
                                low.push(*a);
                            }
                        }
                    }
                }
                for u in &uses {
                    if k[u.0 as usize] != KL {
                        continue;
                    }
                    match low.iter().position(|l| l == u) {
                        Some(p) => {
                            low.swap_remove(p);
                        }
                        None => {
                            k[u.0 as usize] = 0;
                            changed = true;
                        }
                    }
                }
            }
            let mut tu: Vec<VReg> = Vec::new();
            match &b.term {
                Term::Branch(c, ..) | Term::Switch(c, _) => tu.push(*c),
                Term::Ret(v) => tu.extend(v.iter().copied()),
                _ => {}
            }
            for u in tu {
                if k[u.0 as usize] == KL {
                    k[u.0 as usize] = 0;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    k
}

/// The operands instruction `i` reads as their low 32 bits (wasm32 addresses, narrow stores and
/// vector lanes, and the operands `Fx::expr32` reads for a narrow result), given the narrow kinds.
fn low_uses(i: &Inst, k: &[u8], out: &mut Vec<VReg>) {
    let narrow = |d: VReg| k[d.0 as usize] != 0;
    let small = |m: Mem| matches!(m, Mem::Int(1 | 2 | 4, _));
    match *i {
        Inst::Load(_, d, base, _) => {
            out.push(base);
            let _ = d;
        }
        Inst::VLoad(_, base, _) | Inst::AtomicLoad(_, _, _, base) | Inst::AtomicStore(_, _, base, _) | Inst::AtomicRmw(_, _, _, _, base, _) | Inst::AtomicCas(_, _, _, _, base, _, _) | Inst::VStore(base, _, _) => out.push(base),
        Inst::LoadX(_, _, base, idx, _, _) | Inst::VLoadX(_, base, idx, _, _) | Inst::VStoreX(base, idx, _, _, _) => out.extend([base, idx]),
        Inst::Store(m, base, _, src) => {
            out.push(base);
            if small(m) {
                out.push(src);
            }
        }
        Inst::StoreX(m, base, idx, _, _, src) => {
            out.extend([base, idx]);
            if small(m) {
                out.push(src);
            }
        }
        Inst::Copy(dst, src, _) => out.extend([dst, src]),
        Inst::VSplat(_, x) | Inst::VInsert(_, _, _, x) => out.push(x),
        Inst::VX(VX::Splat(w), _, x, _) if w != VW::D => out.push(x),
        Inst::VX(VX::Insert(w, _), _, _, y) if w != VW::D => out.push(y),
        Inst::Conv(Conv::BitsToF(false), _, x) => out.push(x),
        Inst::Call(Callee::Indirect(x) | Callee::CIndirect(x, _), _, _) => out.push(x),
        _ => {}
    }
    let Some(d) = tree_def(i) else { return };
    if !narrow(d) {
        return;
    }
    let low_op = |op: IOp| matches!(op, IOp::Add | IOp::Sub | IOp::Mul | IOp::And | IOp::Or | IOp::Xor);
    match *i {
        Inst::Mov(_, a) | Inst::INeg(_, _, a) | Inst::INot(_, _, a) | Inst::Conv(Conv::IntToInt(_), _, a) => out.push(a),
        Inst::IBin(op, it, _, a, x) if low_op(op) || (it.bits == 32 && !it.signed && matches!(op, IOp::Shl | IOp::Shr | IOp::Rotl)) => out.extend([a, x]),
        Inst::IBinI(op, it, _, a, imm) if low_op(op) || (it.bits == 32 && !it.signed && matches!(op, IOp::Shl | IOp::Shr | IOp::Rotl)) || (op == IOp::Shl && (0..32).contains(&imm)) => out.push(a),
        Inst::Sel(_, _, x, y) => out.extend([x, y]),
        _ => {}
    }
}

const NONE: u32 = u32::MAX;
const DISPATCH: u32 = u32::MAX - 1;

struct Fx<'a> {
    o: &'a Options,
    f: &'a Func,
    fn_ix: &'a HashMap<u32, u32>,
    funcs: &'a [(u32, &'a Func)],
    link: &'a dyn Link,
    sh: &'a mut Shared,
    out: Vec<u8>,
    fix: Vec<(u32, Fix)>,
    cfg: Cfg,
    ctx: Vec<Ctx>,
    /// local per vreg (NONE: folded or never touched)
    loc: Vec<u32>,
    /// per block: per inst, folded into its user
    folded: Vec<Vec<bool>>,
    /// per vreg: (block, inst) of its folded definition
    fold_def: Vec<(u32, u32)>,
    uses: Vec<u32>,
    slot_at: Vec<i64>,
    frame: u32,
    fp: u32,
    tmp64: u32,
    lbl: u32,
    dispatch: bool,
    /// returns branch to one exit block that restores the shadow stack pointer (a frame and two
    /// or more returns): `br` instead of each return's `global.set` + `return`
    exit: bool,
    /// ... and its branches carry the function's result
    exit_vals: bool,
    last_br0: usize,
    /// (start, end, depth) of the last unconditional `br`
    last_br: (usize, usize, u32),
    /// per vreg: the slot whose SlotAddr is its one definition
    slot_addr: Vec<Option<u32>>,
    last_transfer: usize,
    last_ret: usize,
    /// (end, opcode position, local, vreg) of the last `local.set`
    last_set: (usize, usize, u32, u32),
    sp0: u32,
    /// per vreg: its narrow kind (KZ / KS; 0: a wide i64): narrow values live in i32 locals
    nk: Vec<u8>,
    /// every function's signature facts (by function index), and this function's index
    sigs: &'a [Sig],
    me: usize,
    /// per vreg: every definition is below 32 (a 5-bit mask or such a constant): a shift count
    small5: Vec<bool>,
    /// MAKEPAD_RUN_WASM_EXPORT_FNLIST: code bytes per inline context (rir::pos_ctx of the
    /// statement's position; a block's terminator counts with its last statement)
    ctx_bytes: Option<HashMap<u64, i64>>,
    /// bytes attributed so far (a terminator emits its nested blocks: they claim their own)
    ctx_claimed: i64,
}

/// Whether bodies attribute their bytes to inline contexts (size analysis only).
fn ctx_bytes_on() -> bool {
    static T: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    relax_session::phase::env_once(&T, "MAKEPAD_RUN_WASM_EXPORT_FNLIST").is_some()
}

/// Whether a call's arguments are pushed for the call itself (a direct call of a module
/// function, a bound host symbol, an indirect call): only then may they be expression trees.
fn direct_callee(c: &Callee, fn_ix: &HashMap<u32, u32>, link: &dyn Link) -> bool {
    match c {
        Callee::Fn(id) | Callee::CFn(id, _) => fn_ix.contains_key(id),
        Callee::Host(s) | Callee::CHost(s, _) => !matches!(link.host(s.id), HostBind::Trap),
        Callee::Indirect(_) | Callee::CIndirect(..) => true,
        Callee::Asm(_) => false,
    }
}

/// An instruction's operands in wasm push order, for the ones an expression tree may supply
/// (`invoke_call`: the call an invoke block ends with, whose arguments are not trees).
fn push_operands(i: &Inst, invoke_call: bool, direct: &dyn Fn(&Callee) -> bool) -> Vec<VReg> {
    match i {
        Inst::Mov(_, a) | Inst::INeg(_, _, a) | Inst::INot(_, _, a) | Inst::IBit(_, _, a) | Inst::IBinI(_, _, _, a, _) | Inst::ICmpI(_, _, _, a, _) | Inst::FUnary(_, _, _, a) | Inst::Conv(_, _, a) | Inst::Load(_, _, a, _) | Inst::AtomicLoad(_, _, _, a) => vec![*a],
        Inst::IBin(_, _, _, a, b) | Inst::ICmp(_, _, _, a, b) | Inst::FBin(_, _, _, a, b) | Inst::FCmp(_, _, _, a, b) => vec![*a, *b],
        Inst::Store(_, a, _, s) | Inst::AtomicStore(_, _, a, s) => vec![*a, *s],
        Inst::Copy(d, s, _) => vec![*d, *s],
        Inst::LoadX(_, _, a, x, _, _) => vec![*a, *x],
        Inst::StoreX(_, a, x, _, _, s) => vec![*a, *x, *s],
        Inst::Sel(_, c, a, b) => vec![*b, *a, *c],
        Inst::FMulAdd(_, _, _, _, a, b, c) => vec![*a, *b, *c],
        Inst::VSplat(_, a) | Inst::VExtract(_, a, _) | Inst::VLoad(_, a, _) | Inst::VAny(_, a) => vec![*a],
        Inst::VInsert(_, v, _, s) => vec![*v, *s],
        Inst::VStore(a, _, s) => vec![*a, *s],
        Inst::VLoadX(_, a, x, _, _) => vec![*a, *x],
        Inst::VStoreX(a, x, _, _, s) => vec![*a, *x, *s],
        Inst::VSel(_, m, a, b) => vec![*a, *b, *m],
        Inst::VShuffle(_, a, b, _) => vec![*a, *b],
        Inst::VX(op, _, a, b) => match op {
            VX::Splat(_) | VX::Extract(..) | VX::FromLo64 => vec![*a],
            _ if simd_op(*op).is_some() || matches!(op, VX::Insert(..)) => vec![*a, *b],
            _ => vec![],
        },
        Inst::Call(c, args, _) if !invoke_call => {
            if !direct(c) {
                return vec![];
            }
            let mut v = args.clone();
            if let Callee::Indirect(x) | Callee::CIndirect(x, _) = c {
                v.push(*x);
            }
            v
        }
        _ => vec![],
    }
}

/// The terminator operands an expression tree may supply.
fn term_operands(t: &Term) -> Vec<VReg> {
    match t {
        Term::Branch(c, ..) => vec![*c],
        Term::Ret(v) => v.clone(),
        _ => vec![],
    }
}

/// How far back (instructions) `sink_trees` looks for an operand's definition.
const SINK_WINDOW: usize = 256;

/// Expression-tree scheduling (the stackifier's step before `plan_folds`, as LLVM's
/// WebAssemblyRegStackify): in each block, a value defined once and read once -- by a later
/// instruction of the same block -- moves down to just before its reader's other tree operands,
/// so it is pushed where it is read instead of going through a local. Readers are taken from the
/// block's end, operands in reverse push order (the tree's last operand sits right before its
/// reader). A definition moves past an instruction only when that one writes none of its
/// operands; a load, an integer division or remainder (wasm traps) moves past no call, store or
/// other memory effect; calls, TLS addresses and memory effects never move. Order of effects
/// is unchanged. None: nothing moved.
fn sink_trees(f: &Func, direct: &dyn Fn(&Callee) -> bool) -> Option<Func> {
    let nv = f.vregs.len();
    let mut defs = vec![0u32; nv];
    let mut uses = vec![0u32; nv];
    for p in &f.params {
        defs[p.0 as usize] += 2;
    }
    let mut rd: Vec<VReg> = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            i.defs(|d| defs[d.0 as usize] += 1);
            reads(i, &mut rd);
            for u in &rd {
                uses[u.0 as usize] += 1;
            }
        }
        match &b.term {
            Term::Branch(c, ..) | Term::Switch(c, _) => uses[c.0 as usize] += 1,
            Term::Ret(v) => v.iter().for_each(|r| uses[r.0 as usize] += 1),
            _ => {}
        }
    }
    let movable = |i: &Inst| -> Option<bool> {
        // Some(barrier-sensitive): may move; true: not past a memory effect
        match i {
            Inst::Call(..) | Inst::TlsAddr(..) | Inst::TlsStatic(..) | Inst::AtomicLoad(..) | Inst::Coverage { .. } => None,
            _ if crate::passes::is_mem_effect(i) => None,
            Inst::Load(..) | Inst::LoadX(..) | Inst::VLoad(..) | Inst::VLoadX(..) | Inst::VGather(..) => Some(true),
            Inst::IBin(IOp::Div | IOp::Rem, ..) | Inst::IBinI(IOp::Div | IOp::Rem, ..) => Some(true),
            _ => Some(false),
        }
    };
    let mut out: Option<Func> = None;
    for bi in 0..f.blocks.len() {
        let blk = &f.blocks[bi];
        let n = blk.insts.len();
        if n < 2 {
            continue;
        }
        let invoke = matches!(blk.term, Term::Invoke(..));
        let mut insts: Vec<Inst> = blk.insts.clone();
        let has_pos = blk.pos.len() == n;
        let mut pos: Vec<u64> = if has_pos { blk.pos.clone() } else { Vec::new() };
        let mut moved = false;
        struct S<'a> {
            insts: &'a mut Vec<Inst>,
            pos: &'a mut Vec<u64>,
            defs: &'a [u32],
            uses: &'a [u32],
            n: usize,
            invoke: bool,
            moved: &'a mut bool,
        }
        /// `first`: op is the first value the root statement pushes (the first operand of a
        /// chain of first operands): a value read again later moves right before the statement,
        /// so its `local.set` and this read become one `local.tee`
        fn place(s: &mut S, op: VReg, first: bool, ip: &mut usize, direct: &dyn Fn(&Callee) -> bool, movable: &dyn Fn(&Inst) -> Option<bool>) {
            let multi = s.uses[op.0 as usize] > 1;
            if s.defs[op.0 as usize] != 1 || (multi && !first) || *ip == 0 {
                return;
            }
            let lo = ip.saturating_sub(SINK_WINDOW);
            let Some(j) = (lo..*ip).rev().find(|&k| tree_def(&s.insts[k]) == Some(op)) else { return };
            let movable_j = movable(&s.insts[j]);
            if multi {
                // stays a statement of its own: only moved, never part of this tree
                let Some(barrier) = movable_j else { return };
                if j + 1 == *ip {
                    return;
                }
                let mut ops: Vec<VReg> = Vec::new();
                reads(&s.insts[j], &mut ops);
                let mut rd: Vec<VReg> = Vec::new();
                for k in j + 1..*ip {
                    let x = &s.insts[k];
                    if barrier && crate::passes::is_mem_effect(x) {
                        return;
                    }
                    let mut clash = false;
                    x.defs(|d| clash |= ops.contains(&d));
                    reads(x, &mut rd);
                    if clash || rd.contains(&op) {
                        return;
                    }
                }
                let i = s.insts.remove(j);
                s.insts.insert(*ip - 1, i);
                if !s.pos.is_empty() {
                    let p = s.pos.remove(j);
                    s.pos.insert(*ip - 1, p);
                }
                *s.moved = true;
                return;
            }
            let Some(barrier) = movable_j else {
                if j + 1 != *ip {
                    return;
                }
                // a call (or other effect) right before its reader is a tree as it stands
                *ip = j;
                let sub = push_operands(&s.insts[j], s.invoke && j + 1 == s.n, direct);
                let mut ip2 = j;
                for (k, o) in sub.iter().enumerate().rev() {
                    place(s, *o, first && k == 0, &mut ip2, direct, movable);
                }
                *ip = ip2;
                return;
            };
            if j + 1 != *ip {
                let mut ops: Vec<VReg> = Vec::new();
                reads(&s.insts[j], &mut ops);
                for k in j + 1..*ip {
                    let x = &s.insts[k];
                    if barrier && crate::passes::is_mem_effect(x) {
                        return;
                    }
                    let mut clash = false;
                    x.defs(|d| clash |= ops.contains(&d));
                    if clash {
                        return;
                    }
                }
                let i = s.insts.remove(j);
                s.insts.insert(*ip - 1, i);
                if !s.pos.is_empty() {
                    let p = s.pos.remove(j);
                    s.pos.insert(*ip - 1, p);
                }
                *s.moved = true;
            }
            *ip -= 1;
            let at = *ip;
            let sub = push_operands(&s.insts[at], false, direct);
            let mut ip2 = at;
            for (k, o) in sub.iter().enumerate().rev() {
                place(s, *o, first && k == 0, &mut ip2, direct, movable);
            }
            *ip = ip2;
        }
        let mut s = S { insts: &mut insts, pos: &mut pos, defs: &defs, uses: &uses, n, invoke, moved: &mut moved };
        let mut ip = n;
        if !invoke {
            for (k, op) in term_operands(&blk.term).iter().enumerate().rev() {
                place(&mut s, *op, k == 0, &mut ip, direct, &movable);
            }
        }
        while ip > 0 {
            let c = ip - 1;
            let ops = push_operands(&s.insts[c], invoke && c + 1 == n, direct);
            let mut ip2 = c;
            for (k, op) in ops.iter().enumerate().rev() {
                place(&mut s, *op, k == 0, &mut ip2, direct, &movable);
            }
            ip = ip2;
        }
        if moved {
            let g = out.get_or_insert_with(|| f.clone());
            g.blocks[bi].insts = insts;
            if has_pos {
                g.blocks[bi].pos = pos;
            }
        }
    }
    out
}

/// The vregs instruction `i` reads (its operands; a vector lane insert reads its vector too).
fn reads(i: &Inst, out: &mut Vec<VReg>) {
    out.clear();
    let mut ds: Vec<VReg> = Vec::new();
    i.defs(|d| ds.push(d));
    let mut x = i.clone();
    x.vregs_mut(|v| out.push(*v));
    for d in &ds {
        if let Some(p) = out.iter().position(|v| v == d) {
            out.remove(p);
        }
    }
    if let Inst::VInsert(_, v, _, _) = i {
        if !out.contains(v) {
            out.push(*v);
        }
    }
}

/// The body a function is emitted from: untraced/trap calls end their blocks, vector ops wasm has
/// no instruction for become per-lane scalar code, switches br_table forms. None: `f0` itself.
fn prepare(o: &Options, options: &relax_session::Options, f0: &Func, fn_ix: &HashMap<u32, u32>, link: &dyn Link) -> Option<Func> {
    let native = |op: VX| vx_native(op);
    let pruned: Option<Func> = prune(o, f0, fn_ix, link);
    let f1: &Func = pruned.as_ref().unwrap_or(f0);
    let legal: Option<Func> = crate::legal::needed(f1, &native, true).then(|| {
        let mut g = f1.clone();
        crate::legal::legalize(&mut g, &native, true);
        g
    });
    let f2: &Func = legal.as_ref().unwrap_or(f1);
    // shipping O1 (`full`: the code outside a wasm export's hot set) keeps sparse chains: their
    // binary search is more compares than the chain
    let f3: Option<Func> = match crate::switch::lower_switches_sized(options, f2, crate::switch::MIN_TABLE_CLUSTERS, f2.full) {
        Some(s) => Some(s.0),
        None => legal.or(pruned),
    };
    if !o.sink {
        return f3;
    }
    let base: &Func = f3.as_ref().unwrap_or(f0);
    sink_trees(base, &|c| direct_callee(c, fn_ix, link)).or(f3)
}

/// A function's wasm signature facts: per parameter its narrow kind (0: i64; KZ: an i32 the
/// value zero-extends from; KL: an i32 of the low half, all the body reads), and its single
/// integer result's (0: i64; KZ / KS: an i32 that extends to the value).
#[derive(Clone, Debug, Default)]
struct Sig {
    params: Vec<u8>,
    ret: Vec<u8>,
}

fn func_type_sig(f: &Func, sig: &Sig) -> Result<FnType, String> {
    let mut t = func_type(f)?;
    for (k, p) in t.params.iter_mut().enumerate() {
        if sig.params.get(k).is_some_and(|x| *x != 0) {
            *p = Val::I32;
        }
    }
    for (r, k) in t.results.iter_mut().zip(&sig.ret) {
        if *k != 0 {
            *r = Val::I32;
        }
    }
    Ok(t)
}

fn lower_func(o: &Options, options: &relax_session::Options, f: &Func, me: usize, fn_ix: &HashMap<u32, u32>, funcs: &[(u32, &Func)], sigs: &[Sig], link: &dyn Link, sh: &mut Shared) -> Result<Body, String> {
    let _ = options;
    let ft = func_type_sig(f, &sigs[me])?;
    let ty = sh.ty(ft.clone());
    let cfg = analyse(f);
    let nv = f.vregs.len();
    let mut fx = Fx {
        o,
        f,
        fn_ix,
        funcs,
        link,
        sh,
        out: Vec::with_capacity(64),
        fix: Vec::new(),
        dispatch: !cfg.reducible,
        cfg,
        ctx: Vec::new(),
        loc: vec![NONE; nv],
        folded: Vec::new(),
        fold_def: vec![(NONE, NONE); nv],
        uses: vec![0; nv],
        slot_at: Vec::new(),
        frame: 0,
        fp: NONE,
        tmp64: NONE,
        lbl: NONE,
        exit: false,
        exit_vals: false,
        last_br0: usize::MAX,
        last_br: (usize::MAX, usize::MAX, 0),
        slot_addr: {
            let mut n = vec![0u32; f.vregs.len()];
            let mut sa = vec![None; f.vregs.len()];
            for p in &f.params { n[p.0 as usize] += 1; }
            for b in &f.blocks {
                for i in &b.insts {
                    i.defs(|d| n[d.0 as usize] += 1);
                    if let Inst::SlotAddr(d, sl) = i { sa[d.0 as usize] = Some(*sl); }
                }
            }
            sa.iter().zip(&n).map(|(s, n)| if *n == 1 { *s } else { None }).collect()
        },
        last_transfer: usize::MAX,
        last_ret: usize::MAX,
        last_set: (usize::MAX, 0, 0, 0),
        sp0: NONE,
        nk: Vec::new(),
        sigs,
        me,
        small5: Vec::new(),
        ctx_bytes: ctx_bytes_on().then(HashMap::new),
        ctx_claimed: 0,
    };
    fx.check()?;
    {
        let mut sm = vec![true; nv];
        let mut seen = vec![false; nv];
        for p in &f.params {
            sm[p.0 as usize] = false;
        }
        for b in &f.blocks {
            for i in &b.insts {
                let ok = matches!(i, Inst::IBinI(IOp::And, _, _, _, m) if (0..32).contains(m)) || matches!(i, Inst::Iconst(_, k) if (0..32).contains(k));
                i.defs(|d| {
                    seen[d.0 as usize] = true;
                    sm[d.0 as usize] &= ok;
                });
            }
        }
        fx.small5 = (0..nv).map(|v| sm[v] && seen[v]).collect();
    }
    {
        let csig = |id: u32| fn_ix.get(&id).map(|&u| &sigs[u as usize]);
        fx.nk = narrow_kinds(f, o, link, &sigs[me].params, &csig);
    }
    for (k, p) in f.params.iter().enumerate() {
        if fx.nk[p.0 as usize] != sigs[me].params[k] {
            return Err(format!("parameter {} narrowed to {} in the body, {} in the signature", k, fx.nk[p.0 as usize], sigs[me].params[k]));
        }
    }
    // the frame first: rematerialising a slot address prices its offset (plan_remat)
    let (slot_at, end) = crate::frame::slot_layout(f, 0, true);
    let frame = ((end + 15) & !15) as u32;
    fx.slot_at = slot_at;
    fx.frame = frame;
    fx.plan_folds();
    let has_invoke = f.blocks.iter().any(|b| matches!(b.term, Term::Invoke(..)));
    let n_ret = fx.cfg.rpo.iter().filter(|b| matches!(f.blocks[**b as usize].term, Term::Ret(_))).count();
    let want_sp0 = o.sp0 && frame > 0 && n_ret >= 2;
    let locals = fx.colour(frame > 0 || has_invoke, want_sp0)?;
    // locals declaration
    let mut body = Vec::new();
    let mut runs: Vec<(u32, Val)> = Vec::new();
    for v in &locals {
        match runs.last_mut() {
            Some((n, t)) if t == v => *n += 1,
            _ => runs.push((1, *v)),
        }
    }
    uleb(&mut body, runs.len() as u64);
    for (n, t) in &runs {
        uleb(&mut body, *n as u64);
        body.push(*t as u8);
    }
    let hdr = body.len() as u32;
    // prologue
    if frame > 0 {
        fx.out.extend_from_slice(&[GGET, 0]);
        if fx.sp0 != NONE {
            fx.out.push(LTEE);
            let l = fx.sp0;
            uleb(&mut fx.out, l as u64);
        }
        fx.aconst(frame as i64);
        fx.a_op(I32_SUB, I64_SUB);
        fx.out.push(LTEE);
        let fp = fx.fp;
        uleb(&mut fx.out, fp as u64);
        fx.out.extend_from_slice(&[GSET, 0]);
    } else if has_invoke {
        fx.out.extend_from_slice(&[GGET, 0, LSET]);
        let fp = fx.fp;
        uleb(&mut fx.out, fp as u64);
    }
    // one exit for every return of a function with a frame (results of at most one value: a
    // block type is a value type then)
    fx.exit = frame > 0 && fx.sp0 == NONE && n_ret >= 2 && ft.results.len() <= 1;
    if fx.exit {
        fx.exit_vals = !ft.results.is_empty();
        fx.op(BLOCK);
        fx.op(ft.results.first().map_or(0x40, |v| *v as u8));
        fx.ctx.push(Ctx::Exit);
    }
    if fx.dispatch {
        fx.emit_dispatch()?;
    } else {
        fx.do_tree(0)?;
    }
    if !ft.results.is_empty() && fx.last_transfer != fx.out.len() {
        fx.out.push(0x00);
    }
    if fx.exit {
        // the last return's branch is the fallthrough
        if fx.last_br0 == fx.out.len() {
            fx.out.truncate(fx.out.len() - 2);
        }
        fx.ctx.pop();
        fx.op(END);
        fx.restore_sp();
    }
    // a trailing `return` is the function's end
    if fx.last_ret == fx.out.len() && fx.ctx.is_empty() {
        fx.out.pop();
    }
    fx.out.push(END);
    body.extend_from_slice(&fx.out);
    let fix = fx.fix.iter().map(|(p, x)| (p + hdr, *x)).collect();
    let mut ctx: Vec<(u64, i64)> = fx.ctx_bytes.take().map_or_else(Vec::new, |m| m.into_iter().collect());
    ctx.sort();
    Ok(Body { bytes: body, fix, ty, ctx })
}

/// A call that never returns here (untraced, or bound to a trap) ends its block; with
/// `panic_abort` an invoke is a plain call and a resume traps. None: nothing to change.
fn prune(o: &Options, f: &Func, fn_ix: &HashMap<u32, u32>, link: &dyn Link) -> Option<Func> {
    let traps = |i: &Inst| match i {
        Inst::Call(Callee::Fn(id) | Callee::CFn(id, _), _, _) => !fn_ix.contains_key(id),
        Inst::Call(Callee::Host(s) | Callee::CHost(s, _), _, _) => link.host(s.id) == HostBind::Trap,
        _ => false,
    };
    let cut = |b: &Block| b.insts.iter().position(|i| traps(i)).map(|k| k + 1 < b.insts.len() || !matches!(b.term, Term::Unreachable)).unwrap_or(false);
    let abort = |b: &Block| o.panic_abort && matches!(b.term, Term::Invoke(..) | Term::Resume);
    if !f.blocks.iter().any(|b| cut(b) || abort(b)) {
        return None;
    }
    let mut g = f.clone();
    for b in &mut g.blocks {
        if let Some(k) = b.insts.iter().position(|i| traps(i)) {
            b.insts.truncate(k + 1);
            b.pos.truncate(k + 1);
            b.term = Term::Unreachable;
        }
        if o.panic_abort {
            match b.term {
                Term::Invoke(n, _) => b.term = Term::Jump(n),
                Term::Resume => b.term = Term::Unreachable,
                _ => {}
            }
        }
    }
    Some(g)
}

/// Lane-typed vector ops with a SIMD128 instruction here.
fn vx_native(op: VX) -> bool {
    use VW::*;
    match op {
        VX::Add(_) | VX::Sub(_) | VX::And | VX::Or | VX::Xor | VX::AndNot => true,
        VX::MulLo(w) => matches!(w, H | S),
        VX::FAdd(w, NanBits::Host) | VX::FSub(w, NanBits::Host) | VX::FMul(w, NanBits::Host) | VX::FDiv(w, NanBits::Host) => matches!(w, S | D),
        VX::Splat(_) | VX::Extract(..) | VX::Insert(..) | VX::FromLo64 => true,
        _ => false,
    }
}

fn simd_op(op: VX) -> Option<u32> {
    use VW::*;
    Some(match op {
        VX::Add(B) => 110,
        VX::Add(H) => 142,
        VX::Add(S) => 174,
        VX::Add(D) => 206,
        VX::Sub(B) => 113,
        VX::Sub(H) => 145,
        VX::Sub(S) => 177,
        VX::Sub(D) => 209,
        VX::MulLo(H) => 149,
        VX::MulLo(S) => 181,
        VX::And => 78,
        VX::AndNot => 79,
        VX::Or => 80,
        VX::Xor => 81,
        VX::FAdd(S, _) => 228,
        VX::FSub(S, _) => 229,
        VX::FMul(S, _) => 230,
        VX::FDiv(S, _) => 231,
        VX::FAdd(D, _) => 240,
        VX::FSub(D, _) => 241,
        VX::FMul(D, _) => 242,
        VX::FDiv(D, _) => 243,
        _ => return None,
    })
}

/// i32 load of an n-byte integer (n <= 4): (opcode, alignment exponent)
fn load32_op(n: u8, signed: bool) -> (u8, u32) {
    match (n, signed) {
        (1, true) => (0x2C, 0),
        (1, false) => (0x2D, 0),
        (2, true) => (0x2E, 1),
        (2, false) => (0x2F, 1),
        _ => (0x28, 2),
    }
}

fn icmp_op(c: Cond, signed: bool) -> u8 {
    match (c, signed) {
        (Cond::Eq, _) => 0x51,
        (Cond::Ne, _) => 0x52,
        (Cond::Lt, true) => 0x53,
        (Cond::Lt, false) => 0x54,
        (Cond::Gt, true) => 0x55,
        (Cond::Gt, false) => 0x56,
        (Cond::Le, true) => 0x57,
        (Cond::Le, false) => 0x58,
        (Cond::Ge, true) => 0x59,
        (Cond::Ge, false) => 0x5A,
    }
}

fn fcmp_op(c: Cond, d64: bool) -> u8 {
    let b = match c {
        Cond::Eq => 0,
        Cond::Ne => 1,
        Cond::Lt => 2,
        Cond::Gt => 3,
        Cond::Le => 4,
        Cond::Ge => 5,
    };
    if d64 { 0x61 + b } else { 0x5B + b }
}

fn inv(c: Cond) -> Cond {
    match c {
        Cond::Eq => Cond::Ne,
        Cond::Ne => Cond::Eq,
        Cond::Lt => Cond::Ge,
        Cond::Ge => Cond::Lt,
        Cond::Le => Cond::Gt,
        Cond::Gt => Cond::Le,
    }
}

/// The single vreg an instruction defines when it may be an expression-tree node.
fn tree_def(i: &Inst) -> Option<VReg> {
    match i {
        Inst::Iconst(d, _) | Inst::Fconst(d, _, _) | Inst::Mov(d, _) | Inst::IBin(_, _, d, _, _) | Inst::IBinI(_, _, d, _, _) | Inst::INeg(_, d, _) | Inst::INot(_, d, _) | Inst::IBit(_, d, _) | Inst::ICmp(_, _, d, _, _) | Inst::ICmpI(_, _, d, _, _) => Some(*d),
        Inst::FBin(_, _, d, _, _) | Inst::FUnary(_, _, d, _) | Inst::FCmp(_, _, d, _, _) | Inst::Conv(_, d, _) | Inst::Load(_, d, _, _) | Inst::LoadX(_, d, _, _, _, _) | Inst::SlotAddr(d, _) | Inst::Addr(d, _) | Inst::FnAddr(d, _) | Inst::TlsAddr(d, _) | Inst::TlsStatic(d, _) => Some(*d),
        Inst::Sel(d, _, _, _) | Inst::FMulAdd(_, _, _, d, _, _, _) | Inst::AtomicLoad(_, _, d, _) => Some(*d),
        Inst::VConst(d, _) | Inst::VSplat(d, _) | Inst::VExtract(d, _, _) | Inst::VInsert(d, _, _, _) | Inst::VLoad(d, _, _) | Inst::VLoadX(d, _, _, _, _) | Inst::VAny(d, _) | Inst::VSel(d, _, _, _) | Inst::VShuffle(d, _, _, _) => Some(*d),
        Inst::VX(op, d, _, _) if simd_op(*op).is_some() || matches!(op, VX::Splat(_) | VX::Extract(..) | VX::Insert(..) | VX::FromLo64) => Some(*d),
        Inst::Call(_, _, r) if r.len() == 1 => Some(r[0]),
        _ => None,
    }
}

fn pure(i: &Inst) -> bool {
    !matches!(i, Inst::Call(..) | Inst::Load(..) | Inst::LoadX(..) | Inst::AtomicLoad(..) | Inst::VLoad(..) | Inst::VLoadX(..))
}

impl<'a> Fx<'a> {
    fn check(&self) -> Result<(), String> {
        for c in &self.f.vregs {
            val_of(*c)?;
        }
        Ok(())
    }

    /// Operands in wasm push order, for the ones an expression tree may supply.
    fn push_order(&self, b: u32, i: &Inst, last: bool) -> Vec<VReg> {
        let invoke_call = last && matches!(self.f.blocks[b as usize].term, Term::Invoke(..));
        push_operands(i, invoke_call, &|c| direct_callee(c, self.fn_ix, self.link))
    }

    fn term_order(&self, t: &Term) -> Vec<VReg> {
        match t {
            Term::Branch(c, ..) => vec![*c],
            Term::Ret(v) => v.clone(),
            _ => vec![],
        }
    }

    fn plan_folds(&mut self) {
        let f = self.f;
        let nv = f.vregs.len();
        let mut defs = vec![0u32; nv];
        for p in &f.params {
            defs[p.0 as usize] += 2;
        }
        for &b in &self.cfg.rpo {
            let blk = &f.blocks[b as usize];
            for i in &blk.insts {
                i.defs(|d| defs[d.0 as usize] += 1);
                let mut x = i.clone();
                let mut ds: Vec<VReg> = Vec::new();
                i.defs(|d| ds.push(d));
                // uses = all vregs minus defs (VInsert's d is a def only)
                let mut all: Vec<VReg> = Vec::new();
                x.vregs_mut(|v| all.push(*v));
                for d in &ds {
                    if let Some(p) = all.iter().position(|v| v == d) {
                        all.remove(p);
                    }
                }
                for u in all {
                    self.uses[u.0 as usize] += 1;
                }
            }
            match &blk.term {
                Term::Branch(c, ..) | Term::Switch(c, _) => self.uses[c.0 as usize] += 1,
                Term::Ret(v) => v.iter().for_each(|r| self.uses[r.0 as usize] += 1),
                _ => {}
            }
        }
        self.folded = f.blocks.iter().map(|b| vec![false; b.insts.len()]).collect();
        for bi in 0..self.cfg.rpo.len() {
            let b = self.cfg.rpo[bi];
            let blk = &f.blocks[b as usize];
            let n = blk.insts.len();
            let mut cur = n as isize - 1;
            // the terminator is the last root
            let ops = self.term_order(&blk.term);
            if !matches!(blk.term, Term::Invoke(..)) {
                self.fold_ops(b, &ops, &mut cur, &defs);
            }
            while cur >= 0 {
                let i = cur as usize;
                cur -= 1;
                let ops = self.push_order(b, &blk.insts[i], i + 1 == n);
                self.fold_ops(b, &ops, &mut cur, &defs);
            }
        }
        if self.o.remat || self.o.remat_slots {
            self.plan_remat(&defs);
        }
    }

    /// Single-definition values cheaper to recompute at each use than to keep in a local.
    fn plan_remat(&mut self, defs: &[u32]) {
        let f = self.f;
        // uses as a memory base (an address operand folds the frame offset into the access)
        let mut base_uses = vec![0u32; f.vregs.len()];
        for &b in &self.cfg.rpo {
            for i in &f.blocks[b as usize].insts {
                match i {
                    Inst::Load(_, _, a, _) | Inst::Store(_, a, _, _) | Inst::VLoad(_, a, _) | Inst::VStore(a, _, _) | Inst::LoadX(_, _, a, _, _, _) | Inst::StoreX(_, a, _, _, _, _) => base_uses[a.0 as usize] += 1,
                    _ => {}
                }
            }
        }
        let small = |k: i64| -> bool { (-64..64).contains(&k) || (-8192..8192).contains(&k) };
        for &b in &self.cfg.rpo {
            for (i, x) in f.blocks[b as usize].insts.iter().enumerate() {
                if self.folded[b as usize][i] {
                    continue;
                }
                let d = match x {
                    Inst::Iconst(d, k) if self.o.remat && small(*k) => *d,
                    Inst::Addr(d, s) | Inst::TlsAddr(d, s) | Inst::TlsStatic(d, s) if self.o.remat && small(self.link.sym(s.id).wrapping_add(s.off as u64) as i64) => *d,
                    // a frame address (merge_slot_addr_defs puts one definition at the entry, for
                    // the native allocators to rematerialise): recomputed at each use when that is
                    // no bigger than a local -- at a memory access it is the frame pointer and the
                    // access's offset, elsewhere `fp + offset`, against its definition, local.set
                    // and a local.get per use
                    Inst::SlotAddr(d, sl) if self.o.remat_slots && defs[d.0 as usize] == 1 => {
                        let so = self.slot_off(*sl);
                        let add = if so == 0 { 0 } else { 1 + sleb_len(so) };
                        let (uses, base) = (self.uses[d.0 as usize] as i64, base_uses[d.0 as usize] as i64);
                        let local = 2 + add + 2 + 2 * uses;
                        let remat = 2 * uses + (uses - base) * add;
                        if remat > local {
                            continue;
                        }
                        self.folded[b as usize][i] = true;
                        self.fold_def[d.0 as usize] = (b, i as u32);
                        continue;
                    }
                    _ => continue,
                };
                if defs[d.0 as usize] != 1 || self.uses[d.0 as usize] < 2 {
                    continue;
                }
                self.folded[b as usize][i] = true;
                self.fold_def[d.0 as usize] = (b, i as u32);
            }
        }
    }

    fn fold_ops(&mut self, b: u32, ops: &[VReg], cur: &mut isize, defs: &[u32]) {
        let blk = &self.f.blocks[b as usize];
        let n = blk.insts.len();
        for op in ops.iter().rev() {
            if *cur < 0 {
                return;
            }
            let j = *cur as usize;
            let p = &blk.insts[j];
            let ok = tree_def(p) == Some(*op) && self.uses[op.0 as usize] == 1 && defs[op.0 as usize] == 1 && !(j + 1 == n && matches!(blk.term, Term::Invoke(..)));
            if !ok {
                // a later operand read from a local: earlier ones may still be trees
                continue;
            }
            self.folded[b as usize][j] = true;
            self.fold_def[op.0 as usize] = (b, j as u32);
            *cur -= 1;
            let sub = self.push_order(b, p, false);
            self.fold_ops(b, &sub, cur, defs);
        }
    }

    /// Liveness, interference and greedy colouring: a local per non-folded vreg; returns the
    /// declared (non-param) locals' types in index order.
    fn colour(&mut self, want_fp: bool, want_sp0: bool) -> Result<Vec<Val>, String> {
        let f = self.f;
        let nv = f.vregs.len();
        let words = (nv + 63) / 64;
        let nb = f.blocks.len();
        let is_tree = |s: &Self, v: VReg| s.fold_def[v.0 as usize].0 != NONE;
        // per block use/def (upward-exposed uses)
        let mut gen = vec![vec![0u64; words]; nb];
        let mut kill = vec![vec![0u64; words]; nb];
        let set = |bs: &mut Vec<u64>, v: u32| bs[v as usize / 64] |= 1 << (v % 64);
        let has = |bs: &Vec<u64>, v: u32| bs[v as usize / 64] >> (v % 64) & 1 != 0;
        let inst_uses = |i: &Inst| -> (Vec<VReg>, Vec<VReg>) {
            let mut ds: Vec<VReg> = Vec::new();
            i.defs(|d| ds.push(d));
            let mut all: Vec<VReg> = Vec::new();
            let mut x = i.clone();
            x.vregs_mut(|v| all.push(*v));
            for d in &ds {
                if let Some(p) = all.iter().position(|v| v == d) {
                    all.remove(p);
                }
            }
            // a VInsert reads its vector too
            if let Inst::VInsert(_, v, _, _) = i {
                if !all.contains(v) {
                    all.push(*v);
                }
            }
            (ds, all)
        };
        let term_uses = |t: &Term| -> Vec<VReg> {
            match t {
                Term::Branch(c, ..) | Term::Switch(c, _) => vec![*c],
                Term::Ret(v) => v.clone(),
                _ => vec![],
            }
        };
        for &b in &self.cfg.rpo {
            let blk = &f.blocks[b as usize];
            let (g, k) = (&mut gen[b as usize], &mut kill[b as usize]);
            for i in &blk.insts {
                let (ds, us) = inst_uses(i);
                for u in us {
                    if !is_tree(self, u) && !has(k, u.0) {
                        set(g, u.0);
                    }
                }
                for d in ds {
                    if !is_tree(self, d) {
                        set(k, d.0);
                    }
                }
            }
            for u in term_uses(&blk.term) {
                if !is_tree(self, u) && !has(k, u.0) {
                    set(g, u.0);
                }
            }
        }
        let mut live_in = vec![vec![0u64; words]; nb];
        let mut live_out = vec![vec![0u64; words]; nb];
        let mut changed = true;
        while changed {
            changed = false;
            for &b in self.cfg.rpo.iter().rev() {
                let mut out = vec![0u64; words];
                for s in f.blocks[b as usize].term.succs() {
                    for w in 0..words {
                        out[w] |= live_in[s as usize][w];
                    }
                }
                let mut inn = out.clone();
                for w in 0..words {
                    inn[w] = gen[b as usize][w] | (out[w] & !kill[b as usize][w]);
                }
                if inn != live_in[b as usize] {
                    live_in[b as usize] = inn;
                    changed = true;
                }
                live_out[b as usize] = out;
            }
        }
        // interference
        let mut adj: Vec<Vec<u32>> = vec![Vec::new(); nv];
        let mut pref: Vec<Vec<u32>> = vec![Vec::new(); nv];
        let mut touched = vec![false; nv];
        let mut ucount = vec![0u32; nv];
        let nk = &self.nk;
        let vty = |v: usize| -> Result<Val, String> { if nk[v] != 0 { Ok(Val::I32) } else { val_of(f.vregs[v]) } };
        let add_edge = |adj: &mut Vec<Vec<u32>>, a: u32, b: u32| {
            if a != b && f.vregs[a as usize] == f.vregs[b as usize] && (nk[a as usize] != 0) == (nk[b as usize] != 0) {
                adj[a as usize].push(b);
                adj[b as usize].push(a);
            }
        };
        for &b in &self.cfg.rpo {
            let blk = &f.blocks[b as usize];
            let mut live = live_out[b as usize].clone();
            for u in term_uses(&blk.term) {
                if !is_tree(self, u) {
                    set(&mut live, u.0);
                    touched[u.0 as usize] = true;
                    ucount[u.0 as usize] += 1;
                }
            }
            for i in blk.insts.iter().rev() {
                let (ds, us) = inst_uses(i);
                let mov_src = if let Inst::Mov(_, a) = i { Some(a.0) } else { None };
                for d in &ds {
                    if is_tree(self, *d) {
                        continue;
                    }
                    touched[d.0 as usize] = true;
                    ucount[d.0 as usize] += 1;
                    for w in 0..words {
                        let mut x = live[w];
                        while x != 0 {
                            let t = x.trailing_zeros();
                            x &= x - 1;
                            let v = (w * 64) as u32 + t;
                            if Some(v) != mov_src {
                                add_edge(&mut adj, d.0, v);
                            }
                        }
                    }
                    // results defined together interfere
                    for d2 in &ds {
                        if !is_tree(self, *d2) {
                            add_edge(&mut adj, d.0, d2.0);
                        }
                    }
                }
                for d in &ds {
                    if !is_tree(self, *d) {
                        live[d.0 as usize / 64] &= !(1 << (d.0 % 64));
                    }
                }
                if let (Some(s), Inst::Mov(d, _)) = (mov_src, i) {
                    if !is_tree(self, *d) && !is_tree(self, VReg(s)) {
                        pref[d.0 as usize].push(s);
                        pref[s as usize].push(d.0);
                    }
                }
                for u in us {
                    if !is_tree(self, u) {
                        set(&mut live, u.0);
                        touched[u.0 as usize] = true;
                        ucount[u.0 as usize] += 1;
                    }
                }
            }
        }
        // params: live together at entry, with everything live into the entry
        let entry_live = live_in[0].clone();
        for (k, p) in f.params.iter().enumerate() {
            touched[p.0 as usize] = true;
            for q in &f.params[k + 1..] {
                add_edge(&mut adj, p.0, q.0);
            }
            for w in 0..words {
                let mut x = entry_live[w];
                while x != 0 {
                    let t = x.trailing_zeros();
                    x &= x - 1;
                    add_edge(&mut adj, p.0, (w * 64) as u32 + t);
                }
            }
        }
        for a in adj.iter_mut() {
            a.sort_unstable();
            a.dedup();
        }
        // colours: params are colours 0..np (their own types)
        let np = f.params.len();
        let mut colour = vec![NONE; nv];
        let mut ctype: Vec<Val> = Vec::new();
        let mut cuse: Vec<u64> = Vec::new();
        for (k, p) in f.params.iter().enumerate() {
            if colour[p.0 as usize] != NONE {
                return Err("a vreg is two parameters".into());
            }
            colour[p.0 as usize] = k as u32;
            ctype.push(vty(p.0 as usize)?);
            cuse.push(ucount[p.0 as usize] as u64);
        }
        let mut order: Vec<u32> = (0..nv as u32).filter(|v| touched[*v as usize] && colour[*v as usize] == NONE).collect();
        order.sort_by(|a, b| ucount[*b as usize].cmp(&ucount[*a as usize]).then(a.cmp(b)));
        let mut busy: Vec<bool> = Vec::new();
        for v in order {
            let t = vty(v as usize)?;
            busy.clear();
            busy.resize(ctype.len(), false);
            for n in &adj[v as usize] {
                let c = colour[*n as usize];
                if c != NONE {
                    busy[c as usize] = true;
                }
            }
            let mut pick = NONE;
            for p in &pref[v as usize] {
                let c = colour[*p as usize];
                if c != NONE && !busy[c as usize] && ctype[c as usize] == t {
                    pick = c;
                    break;
                }
            }
            if pick == NONE {
                pick = (0..ctype.len() as u32).find(|c| !busy[*c as usize] && ctype[*c as usize] == t).unwrap_or(NONE);
            }
            if pick == NONE {
                pick = ctype.len() as u32;
                ctype.push(t);
                cuse.push(0);
            }
            colour[v as usize] = pick;
            cuse[pick as usize] += ucount[v as usize] as u64;
        }
        // extra locals
        let extra = |ty: Val, uses: u64, ctype: &mut Vec<Val>, cuse: &mut Vec<u64>| -> u32 {
            ctype.push(ty);
            cuse.push(uses);
            ctype.len() as u32 - 1
        };
        let aty = if self.o.memory64 { Val::I64 } else { Val::I32 };
        let fpc = if want_fp { extra(aty, 1 << 20, &mut ctype, &mut cuse) } else { NONE };
        let sp0c = if want_sp0 { extra(aty, 1 << 19, &mut ctype, &mut cuse) } else { NONE };
        let needs_tmp = f.blocks.iter().any(|b| matches!(b.term, Term::Switch(..)) || b.insts.iter().any(|i| matches!(i, Inst::AtomicRmw(..) | Inst::AtomicCas(..))));
        let tmpc = if needs_tmp { extra(Val::I64, 1, &mut ctype, &mut cuse) } else { NONE };
        let lblc = if self.dispatch { extra(Val::I32, 1 << 10, &mut ctype, &mut cuse) } else { NONE };
        // final indices: params keep 0..np; the rest grouped by type, groups and members by use
        let nc = ctype.len();
        let mut rest: Vec<u32> = (np as u32..nc as u32).collect();
        let mut gmax: HashMap<Val, u64> = HashMap::new();
        for c in &rest {
            let e = gmax.entry(ctype[*c as usize]).or_insert(0);
            *e = (*e).max(cuse[*c as usize]);
        }
        rest.sort_by(|a, b| {
            let (ta, tb) = (ctype[*a as usize], ctype[*b as usize]);
            gmax[&tb].cmp(&gmax[&ta]).then(ta.cmp(&tb)).then(cuse[*b as usize].cmp(&cuse[*a as usize])).then(a.cmp(b))
        });
        let mut cmap = vec![0u32; nc];
        for c in 0..np {
            cmap[c] = c as u32;
        }
        for (k, c) in rest.iter().enumerate() {
            cmap[*c as usize] = (np + k) as u32;
        }
        for v in 0..nv {
            if colour[v] != NONE {
                self.loc[v] = cmap[colour[v] as usize];
            }
        }
        self.fp = if fpc != NONE { cmap[fpc as usize] } else { NONE };
        self.sp0 = if sp0c != NONE { cmap[sp0c as usize] } else { NONE };
        self.tmp64 = if tmpc != NONE { cmap[tmpc as usize] } else { NONE };
        self.lbl = if lblc != NONE { cmap[lblc as usize] } else { NONE };
        Ok(rest.iter().map(|c| ctype[*c as usize]).collect())
    }

    // ---- small emitters

    fn op(&mut self, b: u8) {
        self.out.push(b);
    }
    fn u(&mut self, v: u64) {
        uleb(&mut self.out, v);
    }
    fn fc(&mut self, sub: u32) {
        self.out.push(0xFC);
        uleb(&mut self.out, sub as u64);
    }
    fn simd(&mut self, sub: u32) {
        self.out.push(0xFD);
        uleb(&mut self.out, sub as u64);
    }
    fn i64c(&mut self, v: i64) {
        self.out.push(I64C);
        sleb(&mut self.out, v);
    }
    fn i32c(&mut self, v: i32) {
        self.out.push(I32C);
        sleb(&mut self.out, v as i64);
    }
    /// A constant of the address type (i32: wasm32, i64: memory64).
    fn aconst(&mut self, v: i64) {
        if self.o.memory64 { self.i64c(v) } else { self.i32c(v as i32) }
    }
    /// The address type's form of a two-operand op (`I32_ADD` / `I64_ADD`).
    fn a_op(&mut self, op32: u8, op64: u8) {
        self.op(if self.o.memory64 { op64 } else { op32 });
    }
    /// The i64 on the stack as an address (wasm32: wrapped to i32).
    fn a_wrap(&mut self) {
        if !self.o.memory64 {
            self.op(WRAP);
        }
    }
    /// The address on the stack as an i64 (wasm32: zero-extended).
    fn a_ext(&mut self) {
        if !self.o.memory64 {
            self.op(EXT_U);
        }
    }
    fn lget(&mut self, l: u32) {
        self.out.push(LGET);
        uleb(&mut self.out, l as u64);
    }
    fn lset(&mut self, l: u32) {
        self.out.push(LSET);
        uleb(&mut self.out, l as u64);
    }
    fn call(&mut self, r: FRef) {
        self.out.push(CALL);
        self.fix.push((self.out.len() as u32, Fix::Func(r)));
    }
    fn helper(&mut self, h: Helper) {
        let i = self.sh.helper(h);
        self.call(FRef::Helper(i));
    }
    fn transfer(&mut self) {
        self.last_transfer = self.out.len();
    }
    fn depth(&self, c: Ctx) -> Option<u32> {
        self.ctx.iter().rev().position(|x| *x == c).map(|p| p as u32)
    }
    fn br(&mut self, d: u32) {
        let start = self.out.len();
        self.op(BR);
        self.u(d as u64);
        self.last_br = (start, self.out.len(), d);
        if d == 0 {
            self.last_br0 = self.out.len();
        }
        self.transfer();
    }
    /// The frame slot whose address `v` is, when its one definition is that slot's SlotAddr.
    fn slot_of(&self, v: VReg) -> Option<u32> {
        let (fb, fi) = self.fold_def[v.0 as usize];
        if fb != NONE {
            return match self.f.blocks[fb as usize].insts[fi as usize] {
                Inst::SlotAddr(_, sl) => Some(sl),
                _ => None,
            };
        }
        self.slot_addr[v.0 as usize]
    }

    /// The unconditional `br` the code emitted so far ends with: (start, depth).
    fn tail_br(&self) -> Option<(usize, u32)> {
        (self.last_br.1 == self.out.len()).then_some((self.last_br.0, self.last_br.2))
    }

    /// Removes the emitted bytes `start..end` (an instruction that turned out redundant), keeping
    /// the call fixups and the end markers of what follows in place.
    fn cut(&mut self, start: usize, end: usize) {
        let n = end - start;
        self.out.drain(start..end);
        for (p, _) in &mut self.fix {
            if *p as usize >= end {
                *p -= n as u32;
            }
        }
        let shift = |m: &mut usize| {
            if *m != usize::MAX && *m >= end {
                *m -= n;
            } else if *m != usize::MAX && *m > start {
                *m = usize::MAX;
            }
        };
        shift(&mut self.last_br0);
        shift(&mut self.last_transfer);
        shift(&mut self.last_ret);
        shift(&mut self.last_set.0);
        shift(&mut self.last_set.1);
        shift(&mut self.last_br.0);
        shift(&mut self.last_br.1);
    }

    fn open(&mut self, op: u8, c: Ctx) {
        self.op(op);
        self.op(0x40);
        self.ctx.push(c);
    }
    fn close(&mut self) {
        let c = self.ctx.pop();
        // `br 0` right before the end of the block it leaves is a fallthrough
        if matches!(c, Some(Ctx::Block(_)) | Some(Ctx::Plain)) && self.last_br0 == self.out.len() {
            self.out.truncate(self.out.len() - 2);
            self.last_br0 = usize::MAX;
        }
        self.op(END);
    }

    /// Re-extends the i64 on the stack to integer type `it`.
    fn ext(&mut self, it: IntTy) {
        match (it.bits, it.signed) {
            (64, _) => {}
            (8, true) => self.op(0xC2),
            (16, true) => self.op(0xC3),
            (32, true) => self.op(0xC4),
            (32, false) => {
                self.op(WRAP);
                self.op(EXT_U);
            }
            (b, false) => {
                self.i64c(((1u64 << b) - 1) as i64);
                self.op(I64_AND);
            }
            (b, true) => {
                self.i64c(64 - b as i64);
                self.op(I64_SHL);
                self.i64c(64 - b as i64);
                self.op(I64_SHR_S);
            }
        }
    }

    /// Pushes vreg `v` (from its tree or its local) in the form `want`; Addr returns the memarg
    /// offset.
    fn value(&mut self, _b: u32, v: VReg, want: Want) -> Result<u64, String> {
        let (fb, fi) = self.fold_def[v.0 as usize];
        if fb != NONE {
            return self.expr(fb, fi as usize, want);
        }
        let l = self.loc[v.0 as usize];
        if l != NONE && self.o.tee && self.last_set.0 == self.out.len() && self.last_set.2 == l && self.last_set.3 == v.0 {
            // the value was just stored: keep it on the stack (tee when it is read again)
            let at = self.last_set.1;
            if self.uses[v.0 as usize] == 1 {
                self.out.truncate(at);
            } else {
                self.out[at] = LTEE;
            }
            self.last_set.0 = usize::MAX;
            return self.from_kind(self.nk[v.0 as usize], want);
        }
        if l != NONE && self.nk[v.0 as usize] != 0 {
            self.lget(l);
            return self.from_kind(self.nk[v.0 as usize], want);
        }
        if l == NONE {
            // never written: zero of its class
            match self.f.vregs[v.0 as usize] {
                Cls::I => self.i64c(0),
                Cls::F32 => {
                    self.op(F32C);
                    self.out.extend_from_slice(&[0; 4]);
                }
                Cls::F64 => {
                    self.op(F64C);
                    self.out.extend_from_slice(&[0; 8]);
                }
                _ => {
                    self.i32c(0);
                    self.simd(17);
                }
            }
        } else {
            self.lget(l);
        }
        self.from_nat(want)
    }

    /// Converts the value on the stack to `want`: an i32 of narrow kind `k` (its low 32 bits), or
    /// a natural i64 (k = 0).
    fn from_kind(&mut self, k: u8, want: Want) -> Result<u64, String> {
        if k == 0 {
            return self.from_nat(want);
        }
        if k == KL && matches!(want, Want::Nat | Want::Truthy | Want::Inverted) {
            return Err(format!("a narrow-by-demand value read as {:?}", want));
        }
        match want {
            Want::Low | Want::Truthy => Ok(0),
            Want::Nat => {
                self.op(if k & KZ != 0 { EXT_U } else { EXT_S });
                Ok(0)
            }
            Want::Inverted => {
                self.op(I32_EQZ);
                Ok(0)
            }
            Want::Addr(off) => {
                if (0..=u32::MAX as i64).contains(&off) {
                    Ok(off as u64)
                } else {
                    self.i32c(off as i32);
                    self.op(I32_ADD);
                    Ok(0)
                }
            }
        }
    }

    /// Converts a natural i64 on the stack to `want`.
    fn from_nat(&mut self, want: Want) -> Result<u64, String> {
        match want {
            Want::Nat => Ok(0),
            Want::Low => {
                self.op(WRAP);
                Ok(0)
            }
            Want::Truthy => {
                self.op(I64_EQZ);
                self.op(I32_EQZ);
                Ok(0)
            }
            Want::Inverted => {
                self.op(I64_EQZ);
                Ok(0)
            }
            Want::Addr(off) => {
                if off < 0 || off > u32::MAX as i64 {
                    self.i64c(off);
                    self.op(I64_ADD);
                    self.a_wrap();
                    Ok(0)
                } else {
                    self.a_wrap();
                    Ok(off as u64)
                }
            }
        }
    }

    fn slot_off(&self, s: u32) -> i64 {
        self.frame as i64 - self.slot_at[s as usize]
    }

    fn sym_val(&self, s: Sym) -> i64 {
        self.link.sym(s.id).wrapping_add(s.off as u64) as i64
    }

    fn memarg(&mut self, align: u32, off: u64) {
        self.u(align as u64);
        self.u(off);
    }

    /// Pushes the address `base + (idx << sh)` and returns the offset (`off` added).
    fn addr_x(&mut self, b: u32, base: VReg, idx: VReg, sh: u8, off: i32) -> Result<u64, String> {
        if !self.o.memory64 && (self.nk[base.0 as usize] != 0 || self.nk[idx.0 as usize] != 0) {
            // wasm32: the address is the low 32 bits of base + (idx << sh)
            self.value(b, base, Want::Low)?;
            self.value(b, idx, Want::Low)?;
            if sh > 0 {
                self.i32c(sh as i32);
                self.op(0x74);
            }
            self.op(I32_ADD);
            return self.from_kind(KZ, Want::Addr(off as i64));
        }
        self.value(b, base, Want::Nat)?;
        self.value(b, idx, Want::Nat)?;
        if sh > 0 {
            self.i64c(sh as i64);
            self.op(I64_SHL);
        }
        self.op(I64_ADD);
        self.from_nat(Want::Addr(off as i64))
    }

    fn load_op(&mut self, m: Mem) -> Result<(u8, u32), String> {
        Ok(match m {
            Mem::Int(1, s) => (if s { 0x30 } else { 0x31 }, 0),
            Mem::Int(2, s) => (if s { 0x32 } else { 0x33 }, 1),
            Mem::Int(4, s) => (if s { 0x34 } else { 0x35 }, 2),
            Mem::Int(8, _) => (0x29, 3),
            Mem::F32 => (0x2A, 2),
            Mem::F64 => (0x2B, 3),
            Mem::Int(n, _) => return Err(format!("{}-byte integer access", n)),
        })
    }

    /// Pushes a store's source and returns the store (opcode, alignment): a narrow source of an
    /// integer of at most 4 bytes is stored by an i32 store.
    fn store_src(&mut self, b: u32, m: Mem, src: VReg) -> Result<(u8, u32), String> {
        if let Mem::Int(n @ (1 | 2 | 4), _) = m {
            if self.nk[src.0 as usize] != 0 {
                self.value(b, src, Want::Low)?;
                return Ok(match n { 1 => (0x3A, 0), 2 => (0x3B, 1), _ => (0x36, 2) });
            }
        }
        self.value(b, src, Want::Nat)?;
        self.store_op(m)
    }

    fn store_op(&mut self, m: Mem) -> Result<(u8, u32), String> {
        Ok(match m {
            Mem::Int(1, _) => (0x3C, 0),
            Mem::Int(2, _) => (0x3D, 1),
            Mem::Int(4, _) => (0x3E, 2),
            Mem::Int(8, _) => (0x37, 3),
            Mem::F32 => (0x38, 2),
            Mem::F64 => (0x39, 3),
            Mem::Int(n, _) => return Err(format!("{}-byte integer access", n)),
        })
    }

    fn fconst(&mut self, bits: u64, d64: bool) {
        if d64 {
            let x = f64::from_bits(bits);
            if x.fract() == 0.0 && x.abs() < (1u64 << 31) as f64 && !(x == 0.0 && x.is_sign_negative()) {
                self.i32c(x as i32);
                self.op(0xB7);
            } else if !x.is_nan() && (x as f32) as f64 == x {
                self.op(F32C);
                self.out.extend_from_slice(&(x as f32).to_bits().to_le_bytes());
                self.op(F64_PROMOTE);
            } else {
                self.op(F64C);
                self.out.extend_from_slice(&bits.to_le_bytes());
            }
        } else {
            let x = f32::from_bits(bits as u32);
            if x.fract() == 0.0 && x.abs() < 64.0 && !(x == 0.0 && x.is_sign_negative()) {
                self.i32c(x as i32);
                self.op(0xB2);
            } else {
                self.op(F32C);
                self.out.extend_from_slice(&(bits as u32).to_le_bytes());
            }
        }
    }

    /// Emits instruction (b, i) leaving its single result on the stack as `want`.
    fn expr(&mut self, b: u32, i: usize, want: Want) -> Result<u64, String> {
        let f = self.f;
        let inst = &f.blocks[b as usize].insts[i];
        if let Some(d) = tree_def(inst) {
            let k = self.nk[d.0 as usize];
            // a constant offset into an address: the access's memarg offset
            if let (Inst::IBinI(IOp::Add, it, _, a, imm), Want::Addr(off)) = (inst, want) {
                if it.bits == 64 {
                    return self.value(b, *a, Want::Addr(off.wrapping_add(*imm)));
                }
            }
            // a frame address (narrow or not) at an access: the frame pointer, its offset in the memarg
            if let (Inst::SlotAddr(_, sl), Want::Addr(off)) = (inst, want) {
                let t = self.slot_off(*sl) + off;
                if (0..=u32::MAX as i64).contains(&t) {
                    self.lget(self.fp);
                    return Ok(t as u64);
                }
            }
            if k != 0 {
                // a compare's i32 is its narrow value; branch forms keep their inverted compares
                if matches!(inst, Inst::ICmp(..) | Inst::ICmpI(..) | Inst::FCmp(..)) && matches!(want, Want::Truthy | Want::Inverted) {
                    return self.expr_wide(b, i, want);
                }
                // a narrow constant wanted whole: its i64 form directly, not an i32 extended
                if let (Inst::Iconst(..), Want::Nat) = (inst, want) {
                    return self.expr_wide(b, i, want);
                }
                self.expr32(b, i)?;
                return self.from_kind(k, want);
            }
        }
        self.expr_wide(b, i, want)
    }

    /// Re-extends the low 32 bits on the stack (i32) to integer type `it` (8 / 16 bits; 32 and
    /// 64 need nothing in the low half).
    fn ext32(&mut self, it: IntTy) {
        match (it.bits, it.signed) {
            (8, true) => self.op(0xC0),
            (16, true) => self.op(0xC1),
            (b, false) if b < 32 => {
                self.i32c(((1u64 << b) - 1) as i32);
                self.op(0x71);
            }
            _ => {}
        }
    }

    /// A compare of `a` with `x` (or `imm`) leaving i32 0/1: in 32 bits when both operands
    /// extend the same way from their low halves (the order of zero- or sign-extended values
    /// is their low halves' unsigned order; sign-extended ones also keep the signed order).
    fn icmp(&mut self, b: u32, c: Cond, signed: bool, a: VReg, x: Option<VReg>, imm: i64) -> Result<(), String> {
        let ka = self.nk[a.0 as usize] & (KZ | KS);
        let kx = match x {
            Some(x) => self.nk[x.0 as usize] & (KZ | KS),
            None => const_kind(imm),
        };
        let k = ka & kx;
        let w32 = ka != 0 && if signed && !matches!(c, Cond::Eq | Cond::Ne) { k & KS != 0 } else { k != 0 };
        if w32 {
            self.value(b, a, Want::Low)?;
            match x {
                Some(x) => {
                    self.value(b, x, Want::Low)?;
                }
                None if imm == 0 && c == Cond::Eq => {
                    self.op(I32_EQZ);
                    return Ok(());
                }
                None => self.i32c(imm as i32),
            }
            self.op(icmp_op(c, signed) - 11);
            return Ok(());
        }
        self.value(b, a, Want::Nat)?;
        match x {
            Some(x) => {
                self.value(b, x, Want::Nat)?;
            }
            None if imm == 0 && c == Cond::Eq => {
                self.op(I64_EQZ);
                return Ok(());
            }
            None => self.i64c(imm),
        }
        self.op(icmp_op(c, signed));
        Ok(())
    }

    /// `x` is a shift count below 32: every definition masks it to 5 bits or is such a constant.
    fn small_count(&self, x: VReg) -> bool {
        self.small5[x.0 as usize]
    }

    /// Emits instruction (b, i), whose result is a narrow vreg, leaving its low 32 bits (i32).
    fn expr32(&mut self, b: u32, i: usize) -> Result<(), String> {
        let inst = self.f.blocks[b as usize].insts[i].clone();
        let kz = |s: &Self, v: VReg| s.nk[v.0 as usize] & KZ != 0;
        let ks = |s: &Self, v: VReg| s.nk[v.0 as usize] & KS != 0;
        // add, sub, mul and the bitwise ops: the result's low half is the op of the low halves
        let low_op = |op: IOp| -> Option<u8> {
            Some(match op {
                IOp::Add => 0x6A,
                IOp::Sub => 0x6B,
                IOp::Mul => 0x6C,
                IOp::And => 0x71,
                IOp::Or => 0x72,
                IOp::Xor => 0x73,
                _ => return None,
            })
        };
        let bitwise = |op: IOp| matches!(op, IOp::And | IOp::Or | IOp::Xor);
        match &inst {
            Inst::Iconst(_, v) => self.i32c(*v as i32),
            Inst::Mov(_, a) => {
                self.value(b, *a, Want::Low)?;
            }
            Inst::IBin(op, it, _, a, x) if low_op(*op).is_some() => {
                self.value(b, *a, Want::Low)?;
                self.value(b, *x, Want::Low)?;
                self.op(low_op(*op).unwrap());
                if !bitwise(*op) {
                    self.ext32(*it);
                }
            }
            Inst::IBinI(op, it, _, a, imm) if low_op(*op).is_some() => {
                self.value(b, *a, Want::Low)?;
                self.i32c(*imm as i32);
                self.op(low_op(*op).unwrap());
                if !bitwise(*op) {
                    self.ext32(*it);
                }
            }
            // unsigned 32-bit shifts and rotates: i32 ops (the count mod 32), as the wide form
            Inst::IBin(op @ (IOp::Shl | IOp::Shr | IOp::Rotl), it, _, a, x) if it.bits == 32 && !it.signed => {
                self.value(b, *a, Want::Low)?;
                self.value(b, *x, Want::Low)?;
                self.op(match op { IOp::Shl => 0x74, IOp::Shr => 0x76, _ => 0x77 });
            }
            Inst::IBinI(op @ (IOp::Shl | IOp::Shr | IOp::Rotl), it, _, a, imm) if it.bits == 32 && !it.signed => {
                self.value(b, *a, Want::Low)?;
                self.i32c(*imm as i32);
                self.op(match op { IOp::Shl => 0x74, IOp::Shr => 0x76, _ => 0x77 });
            }
            // a count known below 32 (masked by the front end): the i32 shift's
            Inst::IBin(op @ (IOp::Shl | IOp::Shr), it, _, a, x) if self.small_count(*x) && (*op == IOp::Shl || it.signed && it.bits == 32 && ks(self, *a)) => {
                self.value(b, *a, Want::Low)?;
                self.value(b, *x, Want::Low)?;
                self.op(if *op == IOp::Shl { 0x74 } else { 0x75 });
                if *op == IOp::Shl {
                    self.ext32(*it);
                }
            }
            Inst::IBinI(IOp::Shl, it, _, a, imm) if (0..32).contains(imm) => {
                self.value(b, *a, Want::Low)?;
                self.i32c(*imm as i32);
                self.op(0x74);
                self.ext32(*it);
            }
            Inst::IBinI(IOp::Shr, it, _, a, imm) if it.signed && it.bits == 32 && (0..32).contains(imm) && ks(self, *a) => {
                self.value(b, *a, Want::Low)?;
                self.i32c(*imm as i32);
                self.op(0x75);
            }
            Inst::IBin(op @ (IOp::Div | IOp::Rem), it, _, a, x) if it.bits == 32 && !it.signed && kz(self, *a) && kz(self, *x) => {
                self.value(b, *a, Want::Low)?;
                self.value(b, *x, Want::Low)?;
                self.op(if *op == IOp::Div { 0x6E } else { 0x70 });
            }
            Inst::IBinI(op @ (IOp::Div | IOp::Rem), it, _, a, imm) if it.bits == 32 && !it.signed && kz(self, *a) && (1..1i64 << 32).contains(imm) => {
                self.value(b, *a, Want::Low)?;
                self.i32c(*imm as i32);
                self.op(if *op == IOp::Div { 0x6E } else { 0x70 });
            }
            Inst::INeg(it, _, a) => {
                self.i32c(0);
                self.value(b, *a, Want::Low)?;
                self.op(I32_SUB);
                self.ext32(*it);
            }
            Inst::INot(it, _, a) => {
                self.value(b, *a, Want::Low)?;
                self.i32c(-1);
                self.op(0x73);
                self.ext32(*it);
            }
            Inst::ICmp(c, sg, _, a, x) => self.icmp(b, *c, *sg, *a, Some(*x), 0)?,
            Inst::ICmpI(c, sg, _, a, imm) => self.icmp(b, *c, *sg, *a, None, *imm)?,
            Inst::Load(Mem::Int(n, sg), _, base, off) if *n <= 4 => {
                let o = self.value(b, *base, Want::Addr(*off as i64))?;
                let (op, al) = load32_op(*n, *sg);
                self.op(op);
                self.memarg(al, o);
            }
            Inst::LoadX(Mem::Int(n, sg), _, base, idx, sh, off) if *n <= 4 => {
                let o = self.addr_x(b, *base, *idx, *sh, *off)?;
                let (op, al) = load32_op(*n, *sg);
                self.op(op);
                self.memarg(al, o);
            }
            Inst::SlotAddr(_, sl) => {
                let so = self.slot_off(*sl);
                self.lget(self.fp);
                if so != 0 {
                    self.i32c(so as i32);
                    self.op(I32_ADD);
                }
            }
            Inst::Addr(_, sy) | Inst::TlsAddr(_, sy) | Inst::TlsStatic(_, sy) => {
                let v = self.sym_val(*sy);
                self.i32c(v as i32);
            }
            Inst::FnAddr(_, id) => {
                let t = table_index(self.sh, self.fn_ix, *id);
                self.i32c(t as i32);
            }
            Inst::Sel(_, c, x, y) => {
                self.value(b, *y, Want::Low)?;
                self.value(b, *x, Want::Low)?;
                self.value(b, *c, Want::Inverted)?;
                self.op(SELECT);
            }
            Inst::Conv(Conv::IntToInt(it), _, x) => {
                self.value(b, *x, Want::Low)?;
                self.ext32(*it);
            }
            Inst::Conv(Conv::FToInt(from64, it), _, x) if it.bits == 32 => {
                self.value(b, *x, Want::Nat)?;
                self.fc(if *from64 { if it.signed { 2 } else { 3 } } else if it.signed { 0 } else { 1 });
            }
            Inst::Conv(Conv::FToBits(false), _, x) => {
                self.value(b, *x, Want::Nat)?;
                self.op(0xBC);
            }
            // a narrow call result is a callee's narrow return (call_inst leaves the i32)
            Inst::Call(..) => {
                let last = i + 1 == self.f.blocks[b as usize].insts.len();
                self.call_inst(b, &inst, last)?;
            }
            _ => {
                self.expr_wide(b, i, Want::Nat)?;
                self.op(WRAP);
            }
        }
        Ok(())
    }

    /// Emits instruction (b, i) leaving its single result on the stack as `want` (a wide result;
    /// narrow operands are extended where an op needs the whole value).
    fn expr_wide(&mut self, b: u32, i: usize, want: Want) -> Result<u64, String> {
        let inst = self.f.blocks[b as usize].insts[i].clone();
        let last = i + 1 == self.f.blocks[b as usize].insts.len();
        let d64 = |f: &Self, v: VReg| f.f.vregs[v.0 as usize] == Cls::F64;
        match &inst {
            Inst::Iconst(_, v) => {
                if let Want::Addr(off) = want {
                    let a = v.wrapping_add(off);
                    if (0..=u32::MAX as i64).contains(&a) {
                        self.aconst(a);
                        return Ok(0);
                    }
                }
                self.i64c(*v);
                self.from_nat(want)
            }
            Inst::Fconst(_, bits, d) => {
                self.fconst(*bits, *d);
                Ok(0)
            }
            Inst::Mov(_, a) => self.value(b, *a, want),
            Inst::IBinI(IOp::Add, it, _, a, imm) if it.bits == 64 && matches!(want, Want::Addr(_)) => {
                let Want::Addr(off) = want else { unreachable!() };
                self.value(b, *a, Want::Addr(off.wrapping_add(*imm)))
            }
            Inst::IBin(op, it, _, a, x) => {
                self.ibin(b, *op, *it, *a, Some(*x), 0)?;
                self.from_nat(want)
            }
            Inst::IBinI(op, it, _, a, imm) => {
                self.ibin(b, *op, *it, *a, None, *imm)?;
                self.from_nat(want)
            }
            Inst::INeg(it, _, a) => {
                self.i64c(0);
                self.value(b, *a, Want::Nat)?;
                self.op(I64_SUB);
                self.ext(*it);
                self.from_nat(want)
            }
            Inst::INot(it, _, a) => {
                self.value(b, *a, Want::Nat)?;
                self.i64c(-1);
                self.op(I64_XOR);
                self.ext(*it);
                self.from_nat(want)
            }
            Inst::IBit(op, _, a) => {
                self.value(b, *a, Want::Nat)?;
                match op {
                    BitOp::Clz => self.op(0x79),
                    BitOp::Ctz => self.op(0x7A),
                    BitOp::Popcnt => self.op(0x7B),
                    BitOp::Bswap => self.helper(Helper::Bswap),
                    BitOp::Rev => self.helper(Helper::Rev),
                }
                self.from_nat(want)
            }
            Inst::ICmp(c, s, _, a, x) => {
                let c = if want == Want::Inverted { inv(*c) } else { *c };
                self.icmp(b, c, *s, *a, Some(*x), 0)?;
                self.cmp_result(want)
            }
            Inst::ICmpI(c, s, _, a, imm) => {
                let c = if want == Want::Inverted { inv(*c) } else { *c };
                self.icmp(b, c, *s, *a, None, *imm)?;
                self.cmp_result(want)
            }
            Inst::FCmp(c, d, _, a, x) => {
                self.value(b, *a, Want::Nat)?;
                self.value(b, *x, Want::Nat)?;
                self.op(fcmp_op(*c, *d));
                if want == Want::Inverted {
                    self.op(I32_EQZ);
                    return Ok(0);
                }
                self.cmp_result(want)
            }
            Inst::FBin(op, d, _, a, x) => {
                self.value(b, *a, Want::Nat)?;
                self.value(b, *x, Want::Nat)?;
                let k: u8 = if *d { 0xA0 } else { 0x92 };
                match op {
                    FOp::Add => self.op(k),
                    FOp::Sub => self.op(k + 1),
                    FOp::Mul => self.op(k + 2),
                    FOp::Div => self.op(k + 3),
                    FOp::Min => self.helper(Helper::FMinNum(*d)),
                    FOp::Max => self.helper(Helper::FMaxNum(*d)),
                    FOp::MinLt => self.helper(Helper::FMinLt(*d)),
                    FOp::MaxGt => self.helper(Helper::FMaxGt(*d)),
                }
                Ok(0)
            }
            Inst::FUnary(op, d, _, a) => {
                self.value(b, *a, Want::Nat)?;
                let k: u8 = if *d { 0x99 } else { 0x8B };
                match op {
                    FUn::Abs => self.op(k),
                    FUn::Neg => self.op(k + 1),
                    FUn::Ceil => self.op(k + 2),
                    FUn::Floor => self.op(k + 3),
                    FUn::Trunc => self.op(k + 4),
                    FUn::RoundEven => self.op(k + 5),
                    FUn::Sqrt => self.op(k + 6),
                    FUn::RoundAway => self.helper(Helper::RoundAway(*d)),
                }
                Ok(0)
            }
            Inst::FMulAdd(np, na, d, _, a, x, c) => {
                if *d {
                    self.value(b, *a, Want::Nat)?;
                    if *np {
                        self.op(0x9A);
                    }
                    self.value(b, *x, Want::Nat)?;
                    self.value(b, *c, Want::Nat)?;
                    if *na {
                        self.op(0x9A);
                    }
                    self.helper(Helper::Fma64);
                } else {
                    // the f32 product is exact in f64; one more rounding of the sum. x * a' then
                    // + c': a NaN operand comes out as the engine's f64 ops pass it on (the first
                    // NaN of x, a', c' on x86-64 hosts, as the native fused op; rir::fma_nan_f32)
                    self.value(b, *x, Want::Nat)?;
                    self.op(F64_PROMOTE);
                    self.value(b, *a, Want::Nat)?;
                    self.op(F64_PROMOTE);
                    if *np {
                        self.op(0x9A);
                    }
                    self.op(0xA2);
                    self.value(b, *c, Want::Nat)?;
                    self.op(F64_PROMOTE);
                    if *na {
                        self.op(0x9A);
                    }
                    self.op(0xA0);
                    self.op(F32_DEMOTE);
                }
                Ok(0)
            }
            // a narrow source's low half: i32 conversions
            Inst::Conv(Conv::IntToF(from, to64), _, s) if from.bits <= 32 && self.nk[s.0 as usize] & (KZ | KS) != 0 => {
                self.value(b, *s, Want::Low)?;
                self.op(match (to64, from.signed) {
                    (true, true) => 0xB7,
                    (true, false) => 0xB8,
                    (false, true) => 0xB2,
                    (false, false) => 0xB3,
                });
                Ok(0)
            }
            Inst::Conv(Conv::BitsToF(false), _, s) if self.nk[s.0 as usize] != 0 => {
                self.value(b, *s, Want::Low)?;
                self.op(0xBE);
                Ok(0)
            }
            Inst::Conv(cv, _, s) => {
                let sd = d64(self, *s);
                self.value(b, *s, Want::Nat)?;
                match cv {
                    Conv::IntToInt(it) => {
                        self.ext(*it);
                        return self.from_nat(want);
                    }
                    Conv::IntToF(from, to64) => {
                        let u = from.bits == 64 && !from.signed;
                        self.op(match (to64, u) {
                            (true, false) => 0xB9,
                            (true, true) => 0xBA,
                            (false, false) => 0xB4,
                            (false, true) => 0xB5,
                        });
                    }
                    Conv::FToInt(from64, to) => {
                        let _ = sd;
                        match (to.bits, to.signed) {
                            (64, s) => self.fc(if *from64 { if s { 6 } else { 7 } } else if s { 4 } else { 5 }),
                            (32, s) => {
                                self.fc(if *from64 { if s { 2 } else { 3 } } else if s { 0 } else { 1 });
                                self.op(if s { EXT_S } else { EXT_U });
                            }
                            (bits @ (8 | 16), s) => {
                                let (lo, hi) = if s { (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1) } else { (0, (1i64 << bits) - 1) };
                                if *from64 {
                                    self.fconst((hi as f64).to_bits(), true);
                                    self.op(0xA4);
                                    if s {
                                        self.fconst((lo as f64).to_bits(), true);
                                        self.op(0xA5);
                                    }
                                    self.fc(if s { 2 } else { 3 });
                                } else {
                                    self.op(F32C);
                                    self.out.extend_from_slice(&(hi as f32).to_bits().to_le_bytes());
                                    self.op(0x96);
                                    if s {
                                        self.op(F32C);
                                        self.out.extend_from_slice(&(lo as f32).to_bits().to_le_bytes());
                                        self.op(0x97);
                                    }
                                    self.fc(if s { 0 } else { 1 });
                                }
                                self.op(if s { EXT_S } else { EXT_U });
                            }
                            _ => return Err(format!("float to {}-bit integer", to.bits)),
                        }
                        return self.from_nat(want);
                    }
                    Conv::F32ToF64 => self.op(F64_PROMOTE),
                    Conv::F64ToF32 => self.op(F32_DEMOTE),
                    Conv::BitsToF(true) => self.op(0xBF),
                    Conv::BitsToF(false) => {
                        self.op(WRAP);
                        self.op(0xBE);
                    }
                    Conv::FToBits(true) => {
                        self.op(0xBD);
                        return self.from_nat(want);
                    }
                    Conv::FToBits(false) => {
                        self.op(0xBC);
                        self.op(EXT_U);
                        return self.from_nat(want);
                    }
                }
                Ok(0)
            }
            Inst::Load(m, _, base, off) => {
                let o = self.value(b, *base, Want::Addr(*off as i64))?;
                let (op, al) = self.load_op(*m)?;
                self.op(op);
                self.memarg(al, o);
                if matches!(m, Mem::Int(..)) { self.from_nat(want) } else { Ok(0) }
            }
            Inst::AtomicLoad(m, _, _, base) => {
                let o = self.value(b, *base, Want::Addr(0))?;
                let (op, al) = self.load_op(*m)?;
                self.op(op);
                self.memarg(al, o);
                if matches!(m, Mem::Int(..)) { self.from_nat(want) } else { Ok(0) }
            }
            Inst::LoadX(m, _, base, idx, sh, off) => {
                let o = self.addr_x(b, *base, *idx, *sh, *off)?;
                let (op, al) = self.load_op(*m)?;
                self.op(op);
                self.memarg(al, o);
                if matches!(m, Mem::Int(..)) { self.from_nat(want) } else { Ok(0) }
            }
            Inst::SlotAddr(_, s) => {
                let so = self.slot_off(*s);
                if let Want::Addr(off) = want {
                    let t = so + off;
                    if t >= 0 {
                        self.lget(self.fp);
                        return Ok(t as u64);
                    }
                }
                self.lget(self.fp);
                self.a_ext();
                if so != 0 {
                    self.i64c(so);
                    self.op(I64_ADD);
                }
                self.from_nat(want)
            }
            Inst::Addr(_, s) | Inst::TlsAddr(_, s) | Inst::TlsStatic(_, s) => {
                let v = self.sym_val(*s);
                if let Want::Addr(off) = want {
                    let a = v.wrapping_add(off);
                    if (0..=u32::MAX as i64).contains(&a) {
                        self.aconst(a);
                        return Ok(0);
                    }
                }
                self.i64c(v);
                self.from_nat(want)
            }
            Inst::FnAddr(_, id) => {
                let t = table_index(self.sh, self.fn_ix, *id);
                self.i64c(t as i64);
                self.from_nat(want)
            }
            Inst::Sel(_, c, x, y) => {
                self.value(b, *y, Want::Nat)?;
                self.value(b, *x, Want::Nat)?;
                self.value(b, *c, Want::Inverted)?;
                self.op(SELECT);
                if self.f.vregs[x.0 as usize] == Cls::I { self.from_nat(want) } else { Ok(0) }
            }
            Inst::Call(..) => {
                self.call_inst(b, &inst, last)?;
                if let Inst::Call(_, _, r) = &inst {
                    if self.f.vregs[r[0].0 as usize] == Cls::I {
                        return self.from_nat(want);
                    }
                }
                Ok(0)
            }
            Inst::VConst(_, k) => {
                self.i32c(*k as i32);
                self.simd(17);
                Ok(0)
            }
            Inst::VSplat(_, s) => {
                let i = self.f.vregs[s.0 as usize] == Cls::I;
                self.value(b, *s, if i { Want::Low } else { Want::Nat })?;
                if i {
                    self.simd(17);
                } else {
                    self.simd(19);
                }
                Ok(0)
            }
            Inst::VExtract(d, v, lane) => {
                let i = self.f.vregs[d.0 as usize] == Cls::I;
                self.value(b, *v, Want::Nat)?;
                if i {
                    self.simd(27);
                    self.op(*lane);
                    self.op(EXT_S);
                    return self.from_nat(want);
                }
                self.simd(31);
                self.op(*lane);
                Ok(0)
            }
            Inst::VInsert(_, v, lane, s) => {
                let i = self.f.vregs[s.0 as usize] == Cls::I;
                self.value(b, *v, Want::Nat)?;
                self.value(b, *s, if i { Want::Low } else { Want::Nat })?;
                if i {
                    self.simd(28);
                } else {
                    self.simd(32);
                }
                self.op(*lane);
                Ok(0)
            }
            Inst::VLoad(_, base, off) => {
                let o = self.value(b, *base, Want::Addr(*off as i64))?;
                self.simd(0);
                self.memarg(0, o);
                Ok(0)
            }
            Inst::VLoadX(_, base, idx, sh, off) => {
                let o = self.addr_x(b, *base, *idx, *sh, *off)?;
                self.simd(0);
                self.memarg(0, o);
                Ok(0)
            }
            Inst::VAny(_, v) => {
                self.value(b, *v, Want::Nat)?;
                self.simd(83);
                self.cmp_result(want)
            }
            Inst::VSel(_, m, x, y) => {
                self.value(b, *x, Want::Nat)?;
                self.value(b, *y, Want::Nat)?;
                self.value(b, *m, Want::Nat)?;
                self.simd(82);
                Ok(0)
            }
            Inst::VShuffle(_, x, y, idx) => {
                self.value(b, *x, Want::Nat)?;
                self.value(b, *y, Want::Nat)?;
                self.simd(13);
                let mut zero = false;
                for k in idx {
                    if *k >= 32 {
                        zero = true;
                        self.op(0);
                    } else {
                        self.op(*k);
                    }
                }
                if zero {
                    self.simd(12);
                    for k in idx {
                        self.op(if *k >= 32 { 0 } else { 0xFF });
                    }
                    self.simd(78);
                }
                Ok(0)
            }
            Inst::VX(op, d, x, y) => {
                match op {
                    VX::Splat(w) => {
                        self.value(b, *x, if *w == VW::D { Want::Nat } else { Want::Low })?;
                        if *w == VW::D {
                            self.simd(18);
                        } else {
                            self.simd(match w { VW::B => 15, VW::H => 16, _ => 17 });
                        }
                        Ok(0)
                    }
                    VX::FromLo64 => {
                        // the scalar in lane 0 of i64x2, lane 1 zero
                        self.value(b, *x, Want::Nat)?;
                        self.simd(18);
                        self.i64c(0);
                        self.simd(30);
                        self.op(1);
                        Ok(0)
                    }
                    VX::Extract(w, k) => {
                        self.value(b, *x, Want::Nat)?;
                        match w {
                            VW::B => self.simd(22),
                            VW::H => self.simd(25),
                            VW::S => self.simd(27),
                            VW::D => self.simd(29),
                        }
                        self.op(*k);
                        if *w != VW::D {
                            self.op(EXT_U);
                        }
                        let _ = d;
                        self.from_nat(want)
                    }
                    VX::Insert(w, k) => {
                        self.value(b, *x, Want::Nat)?;
                        self.value(b, *y, if *w == VW::D { Want::Nat } else { Want::Low })?;
                        match w {
                            VW::D => self.simd(30),
                            _ => {
                                self.simd(match w { VW::B => 23, VW::H => 26, _ => 28 });
                            }
                        }
                        self.op(*k);
                        Ok(0)
                    }
                    _ => {
                        let s = simd_op(*op).ok_or_else(|| format!("vector op {:?}", op))?;
                        self.value(b, *x, Want::Nat)?;
                        self.value(b, *y, Want::Nat)?;
                        self.simd(s);
                        Ok(0)
                    }
                }
            }
            other => Err(format!("not an expression: {:?}", other)),
        }
    }

    /// The i32 0/1 of a compare on the stack, as `want`.
    fn cmp_result(&mut self, want: Want) -> Result<u64, String> {
        match want {
            Want::Truthy | Want::Inverted | Want::Low => Ok(0),
            _ => {
                self.op(EXT_U);
                self.from_nat(want)
            }
        }
    }

    fn ibin(&mut self, b: u32, op: IOp, it: IntTy, a: VReg, x: Option<VReg>, imm: i64) -> Result<(), String> {
        if op == IOp::Bzhi {
            // a & !(-1 << (x mod 64))
            self.value(b, a, Want::Nat)?;
            self.i64c(-1);
            match x {
                Some(x) => {
                    self.value(b, x, Want::Nat)?;
                }
                None => self.i64c(imm),
            }
            self.op(I64_SHL);
            self.i64c(-1);
            self.op(I64_XOR);
            self.op(I64_AND);
            return Ok(());
        }
        let narrow32 = it.bits == 32 && !it.signed && matches!(op, IOp::Shl | IOp::Shr | IOp::Rotl);
        self.value(b, a, Want::Nat)?;
        if narrow32 {
            self.op(WRAP);
        }
        match x {
            Some(x) => {
                self.value(b, x, Want::Nat)?;
                if narrow32 {
                    self.op(WRAP);
                }
            }
            None => {
                if narrow32 {
                    self.i32c(imm as i32);
                } else {
                    self.i64c(imm);
                }
            }
        }
        if narrow32 {
            self.op(match op {
                IOp::Shl => 0x74,
                IOp::Shr => 0x76,
                _ => 0x77,
            });
            self.op(EXT_U);
            return Ok(());
        }
        match op {
            IOp::Add => self.op(I64_ADD),
            IOp::Sub => self.op(I64_SUB),
            IOp::Mul => self.op(I64_MUL),
            IOp::And => {
                self.op(I64_AND);
                return Ok(());
            }
            IOp::Or => {
                self.op(I64_OR);
                return Ok(());
            }
            IOp::Xor => {
                self.op(I64_XOR);
                return Ok(());
            }
            IOp::Shl => self.op(I64_SHL),
            IOp::Shr => self.op(if it.signed { I64_SHR_S } else { I64_SHR_U }),
            IOp::Div => self.op(if it.signed { 0x7F } else { 0x80 }),
            IOp::Rem => self.op(if it.signed { 0x81 } else { 0x82 }),
            IOp::MulHi => {
                self.helper(Helper::MulHi(it.signed));
                return Ok(());
            }
            IOp::Bzhi => unreachable!("bzhi handled above"),
            IOp::Rotl => {
                if it.bits != 64 {
                    return Err(format!("rotate of {}-bit {}", it.bits, if it.signed { "signed" } else { "unsigned" }));
                }
                self.op(0x89);
                return Ok(());
            }
        }
        self.ext(it);
        Ok(())
    }

    /// A call instruction, leaving its results on the stack (`rets` order).
    fn call_inst(&mut self, b: u32, inst: &Inst, last: bool) -> Result<(), String> {
        let Inst::Call(callee, args, rets) = inst else { unreachable!() };
        let invoke_call = last && matches!(self.f.blocks[b as usize].term, Term::Invoke(..));
        let _ = invoke_call;
        let push_args = |s: &mut Self| -> Result<(), String> {
            for a in args {
                s.value(b, *a, Want::Nat)?;
            }
            Ok(())
        };
        let sig = |s: &Self| -> Result<FnType, String> {
            let mut p = Vec::new();
            for a in args {
                p.push(val_of(s.f.vregs[a.0 as usize])?);
            }
            let mut r = Vec::new();
            for a in rets {
                r.push(val_of(s.f.vregs[a.0 as usize])?);
            }
            Ok(FnType { params: p, results: r })
        };
        let narrow = match callee {
            Callee::CHost(_, s) | Callee::CIndirect(_, s) | Callee::CFn(_, s) => s.ret_narrow,
            _ => None,
        };
        match callee {
            Callee::Asm(_) => return Err("inline assembly (asm!) has no wasm form".to_string()),
            Callee::Fn(id) | Callee::CFn(id, _) => match self.fn_ix.get(id) {
                Some(&u) => {
                    let want = sig(self)?;
                    let have = func_type(self.funcs[u as usize].1)?;
                    if want != have {
                        return Err(format!("call of slot {} ({}) with a signature unlike its body's: call {:?}, body {:?}", id, self.funcs[u as usize].1.name, want, have));
                    }
                    let cs = &self.sigs[u as usize];
                    for (k, a) in args.iter().enumerate() {
                        let narrow = cs.params.get(k).is_some_and(|x| *x != 0);
                        self.value(b, *a, if narrow { Want::Low } else { Want::Nat })?;
                    }
                    self.call(FRef::User(u));
                    // a narrow result into a wide vreg: extended here (the value's whole form)
                    if rets.len() == 1 && cs.ret[0] != 0 && self.nk[rets[0].0 as usize] == 0 {
                        self.op(if cs.ret[0] & KZ != 0 { EXT_U } else { EXT_S });
                    }
                }
                None => {
                    self.untraced(*id);
                    return Ok(());
                }
            },
            Callee::Host(s) | Callee::CHost(s, _) => match self.link.host(s.id) {
                HostBind::Import(m, f) => {
                    let internal = sig(self)?;
                    let cs = match callee { Callee::CHost(_, cs) => Some(cs.as_ref()), _ => None };
                    let t = match cs {
                        Some(cs) => boundary_type(cs, &internal, self.o.memory64)?,
                        None => internal,
                    };
                    let ty = self.sh.ty(t.clone());
                    if let Some(&i) = self.sh.import_ix.get(&(m.clone(), f.clone())) {
                        if self.sh.imports[i as usize].2 != ty {
                            return Err(format!("import {m}.{f} called with conflicting boundary signatures"));
                        }
                    }
                    let i = self.sh.import(&m, &f, ty);
                    for (a, p) in args.iter().zip(&t.params) {
                        let narrow = *p == Val::I32 && self.f.vregs[a.0 as usize] == Cls::I;
                        self.value(b, *a, if narrow { Want::Low } else { Want::Nat })?;
                        if *p == Val::I32 && !narrow { self.op(WRAP); }
                    }
                    self.call(FRef::Import(i));
                    if t.results == [Val::I32] {
                        self.op(if cs.and_then(|s| s.ret_narrow).is_some_and(|it| it.signed) { EXT_S } else { EXT_U });
                    }
                }
                // a memcpy / memmove / memset whose result goes unused: the bulk instruction at the
                // call (memory.copy / memory.fill), its operands at the address width, instead of a
                // call of the helper taking 64-bit arguments
                HostBind::Builtin(bi @ (Builtin::Memcpy | Builtin::Memmove | Builtin::Memset)) if rets.is_empty() && args.len() == 3 => {
                    let aw = if self.o.memory64 { Want::Nat } else { Want::Low };
                    self.value(b, args[0], aw)?;
                    self.value(b, args[1], if bi == Builtin::Memset { Want::Low } else { aw })?;
                    self.value(b, args[2], aw)?;
                    self.out.push(0xFC);
                    if bi == Builtin::Memset {
                        self.out.extend_from_slice(&[11, 0]);
                    } else {
                        self.out.extend_from_slice(&[10, 0, 0]);
                    }
                }
                HostBind::Builtin(bi) => {
                    let t = sig(self)?;
                    let ty = self.sh.ty(t);
                    push_args(self)?;
                    self.helper(Helper::Builtin(bi, ty));
                }
                HostBind::Trap => {
                    self.op(0x00);
                    self.transfer();
                    return Ok(());
                }
            },
            Callee::Indirect(v) | Callee::CIndirect(v, _) => {
                let t = sig(self)?;
                let ty = self.sh.ty(t);
                push_args(self)?;
                self.value(b, *v, Want::Low)?;
                self.op(CALL_INDIRECT);
                self.fix.push((self.out.len() as u32, Fix::Type(ty)));
                self.op(0);
                self.sh.uses_table = true;
            }
        }
        if let (Some(it), 1) = (narrow, rets.len()) {
            if self.f.vregs[rets[0].0 as usize] == Cls::I {
                self.ext(it);
            }
        }
        Ok(())
    }

    fn untraced(&mut self, id: u32) {
        self.sh.untraced_targets.push(id);
        if self.o.untraced_import {
            self.i32c(id as i32);
            self.helper(Helper::Untraced);
        }
        self.op(0x00);
        self.transfer();
    }

    /// Stores the stack top into vreg d (or drops it when d is never read).
    fn set(&mut self, d: VReg) {
        let l = self.loc[d.0 as usize];
        if l == NONE || self.uses[d.0 as usize] == 0 {
            self.op(DROP);
        } else {
            let at = self.out.len();
            self.lset(l);
            self.last_set = (self.out.len(), at, l, d.0);
        }
    }

    /// A root instruction (not folded).
    fn stmt(&mut self, b: u32, i: usize) -> Result<(), String> {
        let inst = self.f.blocks[b as usize].insts[i].clone();
        let last = i + 1 == self.f.blocks[b as usize].insts.len();
        if let Inst::Call(_, _, rets) = &inst {
            let before = self.last_transfer;
            self.call_inst(b, &inst, last)?;
            if self.last_transfer != before && self.last_transfer == self.out.len() {
                return Ok(()); // a trap
            }
            // a callee's narrow results into wide vregs: extended as each leaves the stack (a single
            // result was extended by call_inst)
            let rk: Vec<u8> = match &inst {
                Inst::Call(Callee::Fn(id) | Callee::CFn(id, _), _, _) if rets.len() > 1 => {
                    self.fn_ix.get(id).map_or_else(|| vec![0; rets.len()], |&u| self.sigs[u as usize].ret.clone())
                }
                _ => vec![0; rets.len()],
            };
            for (k, r) in rets.iter().enumerate().rev() {
                if rk.get(k).is_some_and(|x| *x != 0) && self.nk[r.0 as usize] == 0 {
                    self.op(if rk[k] & KZ != 0 { EXT_U } else { EXT_S });
                }
                self.set(*r);
            }
            return Ok(());
        }
        if let Some(d) = tree_def(&inst) {
            if self.uses[d.0 as usize] == 0 && pure(&inst) {
                return Ok(());
            }
            if let Inst::Mov(d, a) = &inst {
                // through folded moves: `d = a = x` with x in d's local is no code
                let mut src = *a;
                loop {
                    let (fb, fi) = self.fold_def[src.0 as usize];
                    if fb == NONE { break; }
                    match self.f.blocks[fb as usize].insts[fi as usize] {
                        Inst::Mov(_, x) => src = x,
                        _ => break,
                    }
                }
                if self.fold_def[src.0 as usize].0 == NONE && self.loc[d.0 as usize] == self.loc[src.0 as usize] && self.loc[d.0 as usize] != NONE {
                    return Ok(());
                }
            }
            let w = if self.nk[d.0 as usize] != 0 { Want::Low } else { Want::Nat };
            let at = self.out.len();
            self.expr(b, i, w)?;
            // the value is its own local already (a move or a no-op narrow conversion of a
            // coalesced register): no code
            let l = self.loc[d.0 as usize];
            if l != NONE && self.uses[d.0 as usize] != 0 && self.out.len() > at && self.out[at] == LGET {
                let mut one = vec![LGET];
                uleb(&mut one, l as u64);
                if self.out[at..] == one[..] {
                    self.out.truncate(at);
                    return Ok(());
                }
            }
            self.set(d);
            return Ok(());
        }
        match &inst {
            Inst::Store(m, base, off, src) => {
                let o = self.value(b, *base, Want::Addr(*off as i64))?;
                let (op, al) = self.store_src(b, *m, *src)?;
                self.op(op);
                self.memarg(al, o);
            }
            Inst::AtomicStore(m, _, base, src) => {
                let o = self.value(b, *base, Want::Addr(0))?;
                self.value(b, *src, Want::Nat)?;
                let (op, al) = self.store_op(*m)?;
                self.op(op);
                self.memarg(al, o);
            }
            Inst::StoreX(m, base, idx, sh, off, src) => {
                let o = self.addr_x(b, *base, *idx, *sh, *off)?;
                let (op, al) = self.store_src(b, *m, *src)?;
                self.op(op);
                self.memarg(al, o);
            }
            Inst::VStore(base, off, src) => {
                let o = self.value(b, *base, Want::Addr(*off as i64))?;
                self.value(b, *src, Want::Nat)?;
                self.simd(11);
                self.memarg(0, o);
            }
            Inst::VStoreX(base, idx, sh, off, src) => {
                let o = self.addr_x(b, *base, *idx, *sh, *off)?;
                self.value(b, *src, Want::Nat)?;
                self.simd(11);
                self.memarg(0, o);
            }
            Inst::Copy(dst, src, n) => {
                // a copy of a frame slot onto itself (a result forwarded into the slot it came
                // from) is no code
                if *n > 0 && !(dst == src || self.slot_of(*dst).is_some_and(|x| Some(x) == self.slot_of(*src))) {
                    self.addr_exact(b, *dst)?;
                    self.addr_exact(b, *src)?;
                    self.aconst(*n as i64);
                    self.fc(10);
                    self.op(0);
                    self.op(0);
                }
            }
            Inst::AtomicRmw(op, m, _, d, addr, src) => {
                // the address (and a min/max source) is read twice: a value just set must stay in
                // its local (the tee shortcut would drop the set for a single counted use)
                self.last_set.0 = usize::MAX;
                let t = self.tmp64;
                let (lop, lal) = self.load_op(*m)?;
                let (sop, sal) = self.store_op(*m)?;
                self.addr_exact(b, *addr)?;
                self.op(lop);
                self.memarg(lal, 0);
                self.lset(t);
                self.addr_exact(b, *addr)?;
                match op {
                    RmwOp::Xchg => {
                        self.value(b, *src, Want::Nat)?;
                    }
                    RmwOp::Add | RmwOp::Sub | RmwOp::And | RmwOp::Or | RmwOp::Xor | RmwOp::Nand => {
                        self.lget(t);
                        self.value(b, *src, Want::Nat)?;
                        match op {
                            RmwOp::Add => self.op(I64_ADD),
                            RmwOp::Sub => self.op(I64_SUB),
                            RmwOp::And => self.op(I64_AND),
                            RmwOp::Or => self.op(I64_OR),
                            RmwOp::Xor => self.op(I64_XOR),
                            _ => {
                                self.op(I64_AND);
                                self.i64c(-1);
                                self.op(I64_XOR);
                            }
                        }
                    }
                    RmwOp::Max | RmwOp::Min | RmwOp::UMax | RmwOp::UMin => {
                        self.lget(t);
                        self.value(b, *src, Want::Nat)?;
                        self.lget(t);
                        self.value(b, *src, Want::Nat)?;
                        self.op(match op {
                            RmwOp::Max => 0x55,
                            RmwOp::Min => 0x53,
                            RmwOp::UMax => 0x56,
                            _ => 0x54,
                        });
                        self.op(SELECT);
                    }
                }
                self.op(sop);
                self.memarg(sal, 0);
                self.lget(t);
                self.set(*d);
            }
            Inst::AtomicCas(m, _, _, d, addr, exp, new) => {
                self.last_set.0 = usize::MAX;
                let t = self.tmp64;
                let (lop, lal) = self.load_op(*m)?;
                let (sop, sal) = self.store_op(*m)?;
                self.addr_exact(b, *addr)?;
                self.op(lop);
                self.memarg(lal, 0);
                self.lset(t);
                self.lget(t);
                self.value(b, *exp, Want::Nat)?;
                self.op(0x51);
                self.op(IF);
                self.op(0x40);
                self.ctx.push(Ctx::If);
                self.addr_exact(b, *addr)?;
                self.value(b, *new, Want::Nat)?;
                self.op(sop);
                self.memarg(sal, 0);
                self.ctx.pop();
                self.op(END);
                self.lget(t);
                self.set(*d);
            }
            Inst::Coverage { .. } => return Err("native coverage probe in wasm output".into()),
            Inst::Fence(..) => {}
            Inst::FpEnv(false, d) => {
                self.i64c(0);
                self.set(*d);
            }
            Inst::FpEnv(true, _) => {}
            Inst::Cpuid(ds, _, _) => {
                for d in ds {
                    self.i64c(0);
                    self.set(*d);
                }
            }
            Inst::Xgetbv(d, _) => {
                self.i64c(0);
                self.set(*d);
            }
            other => return Err(format!("instruction without a wasm form: {:?}", other)),
        }
        Ok(())
    }

    /// Pushes `v` as an address with no memarg offset left over.
    fn addr_exact(&mut self, b: u32, v: VReg) -> Result<(), String> {
        let o = self.value(b, v, Want::Addr(0))?;
        if o != 0 {
            self.aconst(o as i64);
            self.a_op(I32_ADD, I64_ADD);
        }
        Ok(())
    }

    fn emit_block(&mut self, b: u32) -> Result<(), String> {
        let n = self.f.blocks[b as usize].insts.len();
        let invoke = matches!(self.f.blocks[b as usize].term, Term::Invoke(..));
        for i in 0..n {
            if invoke && i + 1 == n {
                break;
            }
            if !self.folded[b as usize][i] {
                let at = (self.out.len(), self.ctx_claimed);
                self.stmt(b, i)?;
                self.note_ctx(b, i, at);
            }
        }
        let at = (self.out.len(), self.ctx_claimed);
        let r = self.term(b);
        self.note_ctx(b, n.saturating_sub(1), at);
        r
    }

    /// Attributes the bytes emitted since `at` (output length, bytes claimed then), less what
    /// nested blocks claimed meanwhile, to the inline context of statement `i` of block `b`.
    fn note_ctx(&mut self, b: u32, i: usize, at: (usize, i64)) {
        if let Some(m) = self.ctx_bytes.as_mut() {
            let ctx = self.f.blocks[b as usize].pos.get(i).map_or(0, |p| crate::rir::pos_ctx(*p));
            let own = (self.out.len() as i64 - at.0 as i64) - (self.ctx_claimed - at.1);
            *m.entry(ctx).or_insert(0) += own;
            self.ctx_claimed += own;
        }
    }

    fn restore_sp(&mut self) {
        if self.sp0 != NONE {
            self.lget(self.sp0);
            self.out.extend_from_slice(&[GSET, 0]);
        } else if self.frame > 0 {
            self.lget(self.fp);
            self.aconst(self.frame as i64);
            self.a_op(I32_ADD, I64_ADD);
            self.out.extend_from_slice(&[GSET, 0]);
        }
    }

    fn term(&mut self, b: u32) -> Result<(), String> {
        let t = self.f.blocks[b as usize].term.clone();
        match &t {
            Term::Jump(x) => self.do_branch(b, *x)?,
            Term::Branch(c, x, y) => self.branch(b, *c, *x, *y)?,
            Term::Ret(vs) => {
                for (k, v) in vs.iter().enumerate() {
                    let narrow = self.sigs[self.me].ret.get(k).is_some_and(|x| *x != 0);
                    self.value(b, *v, if narrow { Want::Low } else { Want::Nat })?;
                }
                if self.exit {
                    let d = self.depth(Ctx::Exit).unwrap();
                    self.br(d);
                } else {
                    self.restore_sp();
                    self.op(RETURN);
                    self.transfer();
                    self.last_ret = self.out.len();
                }
            }
            Term::Unreachable => {
                self.op(0x00);
                self.transfer();
            }
            Term::Resume => {
                self.sh.uses_tag = true;
                self.op(THROW);
                self.op(0);
                self.transfer();
            }
            Term::Invoke(normal, unwind) => {
                self.sh.uses_tag = true;
                let i = self.f.blocks[b as usize].insts.len() - 1;
                if !self.o.exnref {
                    // try { call } catch_all { sp back; unwind path } ; normal path
                    self.open(TRY, Ctx::Plain);
                    self.stmt(b, i)?;
                    self.op(CATCH_ALL);
                    if self.fp != NONE {
                        self.lget(self.fp);
                        self.out.extend_from_slice(&[GSET, 0]);
                    }
                    self.do_branch(b, *unwind)?;
                    self.ctx.pop();
                    self.op(END);
                    return self.do_branch(b, *normal);
                }
                self.open(BLOCK, Ctx::Plain);
                self.op(TRY_TABLE);
                self.op(0x40);
                self.op(1);
                self.op(2);
                self.op(0);
                self.ctx.push(Ctx::Plain);
                self.stmt(b, i)?;
                self.ctx.pop();
                self.op(END);
                self.do_branch(b, *normal)?;
                self.close();
                if self.fp != NONE {
                    self.lget(self.fp);
                    self.out.extend_from_slice(&[GSET, 0]);
                }
                self.do_branch(b, *unwind)?;
            }
            Term::Switch(v, tab) => self.switch(b, *v, tab)?,
        }
        Ok(())
    }

    /// The label depth a branch from `src` to `t` takes, or None when `t`'s code goes inline.
    fn brable(&self, src: u32, t: u32) -> Option<u32> {
        if self.dispatch {
            return None;
        }
        let back = self.cfg.ix[t as usize] <= self.cfg.ix[src as usize];
        if back {
            self.depth(Ctx::Loop(t))
        } else if self.cfg.merge[t as usize] {
            self.depth(Ctx::Block(t))
        } else {
            None
        }
    }

    fn do_branch(&mut self, src: u32, t: u32) -> Result<(), String> {
        if self.dispatch {
            self.i32c(self.cfg.ix[t as usize] as i32);
            self.lset(self.lbl);
            let d = self.depth(Ctx::Loop(DISPATCH)).unwrap();
            self.br(d);
            return Ok(());
        }
        match self.brable(src, t) {
            Some(d) => {
                self.br(d);
                Ok(())
            }
            None => self.do_tree(t),
        }
    }

    fn branch(&mut self, b: u32, c: VReg, t: u32, e: u32) -> Result<(), String> {
        if self.dispatch {
            self.i32c(self.cfg.ix[t as usize] as i32);
            self.i32c(self.cfg.ix[e as usize] as i32);
            self.value(b, c, Want::Truthy)?;
            self.op(SELECT);
            self.lset(self.lbl);
            let d = self.depth(Ctx::Loop(DISPATCH)).unwrap();
            self.br(d);
            return Ok(());
        }
        let (bt, be) = (self.brable(b, t), self.brable(b, e));
        // a plain local tests cheapest inverted (i64.eqz)
        let tree = self.fold_def[c.0 as usize].0 != NONE;
        match (bt, be) {
            (Some(dt), Some(de)) => {
                self.value(b, c, Want::Truthy)?;
                self.op(BR_IF);
                self.u(dt as u64);
                self.br(de);
            }
            (None, Some(de)) => {
                self.value(b, c, Want::Inverted)?;
                self.op(BR_IF);
                self.u(de as u64);
                self.do_tree(t)?;
            }
            (Some(dt), None) => {
                self.value(b, c, Want::Truthy)?;
                self.op(BR_IF);
                self.u(dt as u64);
                self.do_tree(e)?;
            }
            (None, None) => {
                let (first, second, w) = if tree { (t, e, Want::Truthy) } else { (e, t, Want::Inverted) };
                self.value(b, c, w)?;
                self.open(IF, Ctx::If);
                self.do_tree(first)?;
                let then_br = self.tail_br();
                self.op(ELSE);
                self.do_tree(second)?;
                let else_br = self.tail_br();
                self.ctx.pop();
                // both arms leave for the same label beyond the `if` (a merge): one branch after
                // it instead (a fallthrough when the label's block ends there: `close`)
                // (not a return's branch with its result on the stack)
                let carries = |s: &Self, d: u32| s.exit_vals && s.ctx.len() >= d as usize && s.ctx[s.ctx.len() - d as usize] == Ctx::Exit;
                match (then_br, else_br) {
                    (Some((ts, d)), Some((es, d2))) if d == d2 && d >= 1 && !carries(self, d) => {
                        let te = ts + 1 + uleb_len(d as u64);
                        self.out.truncate(es);
                        self.cut(ts, te);
                        self.op(END);
                        self.br(d - 1);
                    }
                    _ => self.op(END),
                }
            }
        }
        Ok(())
    }

    fn switch(&mut self, b: u32, v: VReg, tab: &SwitchTable) -> Result<(), String> {
        // distinct successors placed inline get a block each (innermost = first)
        let inl: Vec<u32> = tab.succ.iter().copied().filter(|s| self.brable(b, *s).is_none()).collect();
        for _ in &inl {
            self.open(BLOCK, Ctx::Plain);
        }
        let len = tab.targets.len() as i64;
        let tree = self.fold_def[v.0 as usize].0 != NONE;
        if tab.min == 0 && !tree && self.nk[v.0 as usize] & (KZ | KS) != 0 {
            // a narrow index: an unsigned i32 bound check (the wide value's low half orders as
            // the value; extended values past u32 are out of range either way)
            let l = self.loc[v.0 as usize];
            self.lget(l);
            self.i32c(len as i32);
            self.lget(l);
            self.i32c(len as i32);
            self.op(0x49);
            self.op(SELECT);
        } else if tab.min == 0 && !tree {
            let l = self.loc[v.0 as usize];
            self.lget(l);
            self.op(WRAP);
            self.i32c(len as i32);
            self.lget(l);
            self.i64c(len);
        } else {
            self.value(b, v, Want::Nat)?;
            if tab.min != 0 {
                self.i64c(tab.min);
                self.op(I64_SUB);
            }
            self.op(LTEE);
            let t = self.tmp64;
            self.u(t as u64);
            self.op(WRAP);
            self.i32c(len as i32);
            self.lget(t);
            self.i64c(len);
        }
        if !(tab.min == 0 && !tree && self.nk[v.0 as usize] & (KZ | KS) != 0) {
            self.op(0x54);
            self.op(SELECT);
        }
        let depth_of = |s: &Self, t: u32| -> u32 {
            match inl.iter().position(|x| *x == t) {
                Some(k) => k as u32,
                None => s.brable(b, t).unwrap(),
            }
        };
        self.op(BR_TABLE);
        self.u(tab.targets.len() as u64);
        for t in tab.targets.clone() {
            let d = depth_of(self, t);
            self.u(d as u64);
        }
        let d = depth_of(self, tab.default);
        self.u(d as u64);
        self.transfer();
        for s in inl.clone() {
            self.ctx.pop();
            self.op(END);
            self.do_branch(b, s)?;
        }
        Ok(())
    }

    fn do_tree(&mut self, x: u32) -> Result<(), String> {
        let mut ys: Vec<u32> = self.cfg.kids[x as usize].iter().copied().filter(|k| self.cfg.merge[*k as usize]).collect();
        ys.sort_by_key(|k| self.cfg.ix[*k as usize]);
        if self.cfg.loop_hdr[x as usize] {
            self.open(LOOP, Ctx::Loop(x));
            self.node_within(x, &ys)?;
            self.ctx.pop();
            self.op(END);
        } else {
            self.node_within(x, &ys)?;
        }
        Ok(())
    }

    fn node_within(&mut self, x: u32, ys: &[u32]) -> Result<(), String> {
        match ys.split_last() {
            Some((&yn, rest)) => {
                self.open(BLOCK, Ctx::Block(yn));
                self.node_within(x, rest)?;
                self.close();
                self.do_tree(yn)
            }
            None => self.emit_block(x),
        }
    }

    fn emit_dispatch(&mut self) -> Result<(), String> {
        let rpo = self.cfg.rpo.clone();
        let n = rpo.len();
        self.open(LOOP, Ctx::Loop(DISPATCH));
        for _ in 0..n {
            self.open(BLOCK, Ctx::Plain);
        }
        self.lget(self.lbl);
        self.op(BR_TABLE);
        self.u(n as u64 - 1);
        for k in 0..n - 1 {
            self.u(k as u64);
        }
        self.u(n as u64 - 1);
        self.transfer();
        for b in rpo {
            self.ctx.pop();
            self.op(END);
            self.emit_block(b)?;
        }
        self.ctx.pop();
        self.op(END);
        Ok(())
    }
}

// ------------------------------------------------------------ helpers

fn helper_body(o: &Options, h: Helper, sh: &mut Shared, fn_ix: &HashMap<u32, u32>) -> Body {
    let _ = fn_ix;
    use Val::*;
    let fv = |d: bool| if d { F64 } else { F32 };
    let mut c: Vec<u8> = Vec::new();
    let mut locals: Vec<(u32, Val)> = Vec::new();
    let mut fix: Vec<(u32, Fix)> = Vec::new();
    let g = |c: &mut Vec<u8>, l: u8| {
        c.push(LGET);
        c.push(l);
    };
    let (params, results): (Vec<Val>, Vec<Val>) = match h {
        Helper::FMinNum(d) | Helper::FMaxNum(d) => {
            // a NaN operand yields the other one
            let t = fv(d);
            let (ne, mm) = if d { (0x62, if matches!(h, Helper::FMinNum(_)) { 0xA4 } else { 0xA5 }) } else { (0x5C, if matches!(h, Helper::FMinNum(_)) { 0x96 } else { 0x97 }) };
            g(&mut c, 1);
            g(&mut c, 0);
            g(&mut c, 0);
            g(&mut c, 1);
            c.push(mm);
            g(&mut c, 1);
            g(&mut c, 1);
            c.push(ne);
            c.push(SELECT); // b is NaN ? a : min(a, b)
            g(&mut c, 0);
            g(&mut c, 0);
            c.push(ne);
            c.push(SELECT); // a is NaN ? b : that
            (vec![t, t], vec![t])
        }
        Helper::FMinLt(d) | Helper::FMaxGt(d) => {
            let t = fv(d);
            let cmp = match (h, d) {
                (Helper::FMinLt(_), true) => 0x63,
                (Helper::FMinLt(_), false) => 0x5D,
                (_, true) => 0x64,
                _ => 0x5E,
            };
            g(&mut c, 0);
            g(&mut c, 1);
            g(&mut c, 0);
            g(&mut c, 1);
            c.push(cmp);
            c.push(SELECT);
            (vec![t, t], vec![t])
        }
        Helper::RoundAway(d) => {
            let t = fv(d);
            locals.push((1, t));
            let (trunc, sub, abs, ge, add, copysign) = if d { (0x9D, 0xA1, 0x99, 0x66, 0xA0, 0xA6) } else { (0x8F, 0x93, 0x8B, 0x60, 0x92, 0x98) };
            // t = trunc(x)
            g(&mut c, 0);
            c.push(trunc);
            c.push(LSET);
            c.push(1);
            // select(t + copysign(1, x), t, |x - t| >= 0.5)
            g(&mut c, 1);
            c.push(I32C);
            c.push(1);
            c.push(if d { 0xB7 } else { 0xB2 });
            g(&mut c, 0);
            c.push(copysign);
            c.push(add);
            g(&mut c, 1);
            g(&mut c, 0);
            g(&mut c, 1);
            c.push(sub);
            c.push(abs);
            if d {
                c.push(F64C);
                c.extend_from_slice(&0.5f64.to_bits().to_le_bytes());
            } else {
                c.push(F32C);
                c.extend_from_slice(&0.5f32.to_bits().to_le_bytes());
            }
            c.push(ge);
            c.push(SELECT);
            (vec![t], vec![t])
        }
        Helper::MulHi(signed) => {
            // 64x64 -> high 64 from 32-bit halves; locals 2..5: lo*lo>>32, mid parts
            locals.push((4, I64));
            let m32 = |c: &mut Vec<u8>| {
                c.push(I64C);
                sleb(c, 0xFFFF_FFFF);
                c.push(I64_AND);
            };
            let s32 = |c: &mut Vec<u8>| {
                c.push(I64C);
                c.push(32);
                c.push(I64_SHR_U);
            };
            // l2 = a_lo * b_hi ; l3 = a_hi * b_lo
            g(&mut c, 0);
            m32(&mut c);
            g(&mut c, 1);
            s32(&mut c);
            c.push(I64_MUL);
            c.push(LSET);
            c.push(2);
            g(&mut c, 0);
            s32(&mut c);
            g(&mut c, 1);
            m32(&mut c);
            c.push(I64_MUL);
            c.push(LSET);
            c.push(3);
            // l4 = (a_lo*b_lo >> 32) + (l2 & M) + (l3 & M)
            g(&mut c, 0);
            m32(&mut c);
            g(&mut c, 1);
            m32(&mut c);
            c.push(I64_MUL);
            s32(&mut c);
            g(&mut c, 2);
            m32(&mut c);
            c.push(I64_ADD);
            g(&mut c, 3);
            m32(&mut c);
            c.push(I64_ADD);
            c.push(LSET);
            c.push(4);
            // hi = a_hi*b_hi + (l2>>32) + (l3>>32) + (l4>>32)
            g(&mut c, 0);
            s32(&mut c);
            g(&mut c, 1);
            s32(&mut c);
            c.push(I64_MUL);
            g(&mut c, 2);
            s32(&mut c);
            c.push(I64_ADD);
            g(&mut c, 3);
            s32(&mut c);
            c.push(I64_ADD);
            g(&mut c, 4);
            s32(&mut c);
            c.push(I64_ADD);
            if signed {
                // - (a < 0 ? b : 0) - (b < 0 ? a : 0)
                g(&mut c, 1);
                g(&mut c, 0);
                c.push(I64C);
                c.push(63);
                c.push(I64_SHR_S);
                c.push(I64_AND);
                c.push(I64_SUB);
                g(&mut c, 0);
                g(&mut c, 1);
                c.push(I64C);
                c.push(63);
                c.push(I64_SHR_S);
                c.push(I64_AND);
                c.push(I64_SUB);
            }
            (vec![I64, I64], vec![I64])
        }
        Helper::Bswap => {
            // ((x & 0x00ff..) << 8 | (x >> 8) & 0x00ff..), then 16, then rotate 32
            let step = |c: &mut Vec<u8>, mask: u64, sh: i64| {
                c.push(LGET);
                c.push(0);
                c.push(I64C);
                sleb(c, mask as i64);
                c.push(I64_AND);
                c.push(I64C);
                sleb(c, sh);
                c.push(I64_SHL);
                c.push(LGET);
                c.push(0);
                c.push(I64C);
                sleb(c, sh);
                c.push(I64_SHR_U);
                c.push(I64C);
                sleb(c, mask as i64);
                c.push(I64_AND);
                c.push(I64_OR);
                c.push(LSET);
                c.push(0);
            };
            step(&mut c, 0x00FF_00FF_00FF_00FF, 8);
            step(&mut c, 0x0000_FFFF_0000_FFFF, 16);
            g(&mut c, 0);
            c.push(I64C);
            c.push(32);
            c.push(0x89);
            (vec![I64], vec![I64])
        }
        Helper::Rev => {
            let step = |c: &mut Vec<u8>, mask: u64, sh: i64| {
                c.push(LGET);
                c.push(0);
                c.push(I64C);
                sleb(c, mask as i64);
                c.push(I64_AND);
                c.push(I64C);
                sleb(c, sh);
                c.push(I64_SHL);
                c.push(LGET);
                c.push(0);
                c.push(I64C);
                sleb(c, sh);
                c.push(I64_SHR_U);
                c.push(I64C);
                sleb(c, mask as i64);
                c.push(I64_AND);
                c.push(I64_OR);
                c.push(LSET);
                c.push(0);
            };
            step(&mut c, 0x5555_5555_5555_5555, 1);
            step(&mut c, 0x3333_3333_3333_3333, 2);
            step(&mut c, 0x0F0F_0F0F_0F0F_0F0F, 4);
            g(&mut c, 0);
            let i = sh.helper(Helper::Bswap);
            c.push(CALL);
            fix.push((c.len() as u32, Fix::Func(FRef::Helper(i))));
            (vec![I64], vec![I64])
        }
        Helper::Fma64 => {
            // a*b + c with the product's error term (Dekker): exact product split, one sum
            // locals: 3 p, 4 e (product error), 5 s, 6 hi_a, 7 lo_a, 8 hi_b, 9 lo_b
            locals.push((7, F64));
            let split = |c: &mut Vec<u8>, x: u8, hi: u8, lo: u8| {
                // t = x * 134217729; hi = t - (t - x); lo = x - hi
                c.push(LGET);
                c.push(x);
                c.push(F64C);
                c.extend_from_slice(&134217729.0f64.to_bits().to_le_bytes());
                c.push(0xA2);
                c.push(LTEE);
                c.push(hi);
                c.push(LGET);
                c.push(hi);
                c.push(LGET);
                c.push(x);
                c.push(0xA1);
                c.push(0xA1);
                c.push(LTEE);
                c.push(hi);
                c.push(LGET);
                c.push(x);
                c.push(LGET);
                c.push(hi);
                c.push(0xA1);
                c.push(LSET);
                c.push(lo);
                c.push(DROP);
            };
            split(&mut c, 0, 6, 7);
            split(&mut c, 1, 8, 9);
            // p = b * a (a NaN operand: b's, a's, then c's first through the engine's f64 ops on
            // x86-64, as the native fused op; rir::fma_nan_f64)
            g(&mut c, 1);
            g(&mut c, 0);
            c.push(0xA2);
            c.push(LSET);
            c.push(3);
            // e = ((hi_a*hi_b - p) + hi_a*lo_b + lo_a*hi_b) + lo_a*lo_b
            g(&mut c, 6);
            g(&mut c, 8);
            c.push(0xA2);
            g(&mut c, 3);
            c.push(0xA1);
            g(&mut c, 6);
            g(&mut c, 9);
            c.push(0xA2);
            c.push(0xA0);
            g(&mut c, 7);
            g(&mut c, 8);
            c.push(0xA2);
            c.push(0xA0);
            g(&mut c, 7);
            g(&mut c, 9);
            c.push(0xA2);
            c.push(0xA0);
            c.push(LSET);
            c.push(4);
            // s = p + c; err = (p - (s - bb)) + (c - bb), bb = s - p; result = s + (err + e)
            g(&mut c, 3);
            g(&mut c, 2);
            c.push(0xA0);
            c.push(LTEE);
            c.push(5);
            // bb in local 6
            g(&mut c, 5);
            g(&mut c, 3);
            c.push(0xA1);
            c.push(LSET);
            c.push(6);
            g(&mut c, 3);
            g(&mut c, 5);
            g(&mut c, 6);
            c.push(0xA1);
            c.push(0xA1);
            g(&mut c, 2);
            g(&mut c, 6);
            c.push(0xA1);
            c.push(0xA0);
            g(&mut c, 4);
            c.push(0xA0);
            c.push(0xA0);
            // exact when the product is finite, else the plain sum: select(exact, p + c, p - p == p - p)
            g(&mut c, 3);
            g(&mut c, 2);
            c.push(0xA0);
            g(&mut c, 3);
            g(&mut c, 3);
            c.push(0xA1);
            g(&mut c, 3);
            g(&mut c, 3);
            c.push(0xA1);
            c.push(0x61);
            c.push(SELECT);
            (vec![F64, F64, F64], vec![F64])
        }
        Helper::Builtin(bi, ty) => {
            let t = sh.types[ty as usize].clone();
            // an i64 argument as an address
            let aw = |c: &mut Vec<u8>, l: u8| {
                c.push(LGET);
                c.push(l);
                if !o.memory64 {
                    c.push(WRAP);
                }
            };
            let (acst, aty) = if o.memory64 { (I64C, I64) } else { (I32C, I32) };
            match bi {
                Builtin::MemorySize | Builtin::MemoryGrow => {
                    // The std ABI uses i64 RIR registers, including wasm32 usize.
                    if bi == Builtin::MemoryGrow {
                        aw(&mut c, 1);
                    }
                    c.extend_from_slice(&[if bi == Builtin::MemoryGrow { 0x40 } else { 0x3f }, 0]);
                    if !o.memory64 {
                        c.push(0xad); // i64.extend_i32_u (including wasm32 usize::MAX)
                    }
                }
                Builtin::AtomicWait32 | Builtin::AtomicNotify => {
                    if o.shared_max_pages.is_some() {
                        aw(&mut c, 0);
                        g(&mut c, 1);
                        c.push(WRAP);
                        if bi == Builtin::AtomicWait32 { g(&mut c, 2); }
                        c.extend_from_slice(&[0xfe, if bi == Builtin::AtomicWait32 { 1 } else { 0 }, 2, 0]);
                        c.push(0xad);
                    } else if bi == Builtin::AtomicNotify {
                        c.extend_from_slice(&[I64C, 0]);
                    } else {
                        aw(&mut c, 0);
                        c.extend_from_slice(&[0x28, 2, 0]); // i32.load
                        g(&mut c, 1);
                        c.extend_from_slice(&[WRAP, 0x46, 0x04, 0x40, 0x00, 0x0b]);
                        c.extend_from_slice(&[I64C, 1]);
                    }
                }
                Builtin::Memcpy | Builtin::Memmove | Builtin::Memset => {
                    aw(&mut c, 0);
                    if bi == Builtin::Memset {
                        g(&mut c, 1);
                        c.push(WRAP);
                    } else {
                        aw(&mut c, 1);
                    }
                    aw(&mut c, 2);
                    c.push(0xFC);
                    c.push(if bi == Builtin::Memset { 11 } else { 10 });
                    c.push(0);
                    if bi != Builtin::Memset {
                        c.push(0);
                    }
                    if !t.results.is_empty() {
                        g(&mut c, 0);
                    }
                }
                Builtin::Nop | Builtin::One => {
                    let k = (bi == Builtin::One) as u8;
                    for r in &t.results {
                        match r {
                            I32 => c.extend_from_slice(&[I32C, k]),
                            I64 => c.extend_from_slice(&[I64C, k]),
                            F32 => {
                                c.push(F32C);
                                c.extend_from_slice(&[0; 4]);
                            }
                            F64 => {
                                c.push(F64C);
                                c.extend_from_slice(&[0; 8]);
                            }
                            V128 => c.extend_from_slice(&[I32C, 0, 0xFD, 17]),
                        }
                    }
                }
                Builtin::KeyCreate => {
                    // the counter is word 0; key k's value is word k
                    let kb = sh.keys_base as i64;
                    locals.push((1, I64));
                    let l = t.params.len() as u8;
                    c.push(acst);
                    sleb(&mut c, kb);
                    c.push(acst);
                    sleb(&mut c, kb);
                    c.extend_from_slice(&[0x29, 3, 0, I64C, 1, I64_ADD, LTEE, l, 0x37, 3, 0]);
                    g(&mut c, l);
                    if t.results.first() == Some(&I32) {
                        c.push(WRAP);
                    }
                }
                Builtin::KeyGet | Builtin::KeySet => {
                    let kb = sh.keys_base;
                    // the key's word: key << 3 (an address)
                    g(&mut c, 0);
                    if o.memory64 {
                        if t.params[0] == I32 {
                            c.push(EXT_U);
                        }
                        c.extend_from_slice(&[I64C, 3, 0x86]);
                    } else {
                        if t.params[0] == I64 {
                            c.push(WRAP);
                        }
                        c.extend_from_slice(&[I32C, 3, 0x74]);
                    }
                    if bi == Builtin::KeySet {
                        g(&mut c, 1);
                        c.push(0x37);
                    } else {
                        c.push(0x29);
                    }
                    c.push(3);
                    uleb(&mut c, kb);
                }
                Builtin::CatchPanic if o.panic_abort => {
                    g(&mut c, 1);
                    g(&mut c, 0);
                    c.push(WRAP);
                    c.push(CALL_INDIRECT);
                    sh.uses_table = true;
                    let fty = sh.ty(FnType { params: vec![I64], results: vec![] });
                    fix.push((c.len() as u32, Fix::Type(fty)));
                    c.push(0);
                    c.extend_from_slice(&[I64C, 0]);
                }
                Builtin::CatchPanic => {
                    sh.uses_tag = true;
                    let fty = sh.ty(FnType { params: vec![I64], results: vec![] });
                    locals.push((1, aty));
                    c.extend_from_slice(&[GGET, 0, LSET, 2]);
                    if o.exnref {
                        c.extend_from_slice(&[BLOCK, 0x40, TRY_TABLE, 0x40, 1, 2, 0]);
                    } else {
                        c.extend_from_slice(&[TRY, 0x40]);
                    }
                    g(&mut c, 1);
                    g(&mut c, 0);
                    c.push(WRAP);
                    c.push(CALL_INDIRECT);
                    sh.uses_table = true;
                    fix.push((c.len() as u32, Fix::Type(fty)));
                    c.push(0);
                    c.push(I64C);
                    c.push(0);
                    c.push(RETURN);
                    if o.exnref {
                        c.push(END);
                    } else {
                        c.push(CATCH_ALL);
                    }
                    c.push(END);
                    c.extend_from_slice(&[LGET, 2, GSET, 0]);
                    c.push(I64C);
                    c.push(1);
                }
            }
            (t.params.clone(), t.results.clone())
        }
        Helper::Untraced => {
            // (id: i32) -> (): the import's message, then a trap
            if o.untraced_import {
                let ty = sh.ty(FnType { params: vec![I32], results: vec![] });
                let i = sh.import("env", "u", ty);
                g(&mut c, 0);
                c.push(CALL);
                fix.push((c.len() as u32, Fix::Func(FRef::Import(i))));
            }
            c.push(0x00);
            (vec![I32], vec![])
        }
    };
    c.push(END);
    let ty = sh.ty(FnType { params, results });
    let mut body = Vec::new();
    uleb(&mut body, locals.len() as u64);
    for (n, t) in &locals {
        uleb(&mut body, *n as u64);
        body.push(*t as u8);
    }
    let hdr = body.len() as u32;
    body.extend_from_slice(&c);
    Body { bytes: body, fix: fix.into_iter().map(|(p, x)| (p + hdr, x)).collect(), ty, ctx: vec![] }
}
