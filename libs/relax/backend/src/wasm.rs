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
    /// the value of symbol `id` as `Inst::Addr` / `Inst::TlsAddr` use it (a linear-memory
    /// address or a plain number; the backend adds `Sym::off`)
    fn sym(&self, id: u32) -> u64;
    /// how a call to host symbol `id` is bound
    fn host(&self, id: u32) -> HostBind;
}

#[derive(Clone, Debug)]
pub struct Options {
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
        Options { stack_size: 1 << 20, heap_pages: 16, untraced_import: true, names: false, export_tag: true, exnref: false, data_first: false, tee: true, remat: true, remat_slots: false, sp0: false, panic_abort: false, order_by_name: true, fold_identical: true, order_by_bytes: true }
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
    /// function bodies folded into an identical one
    pub n_folded: usize,
    /// (slot id, function name, body bytes) of every chosen function
    pub fn_sizes: Vec<(u32, String, usize)>,
}

pub struct Output {
    pub bytes: Vec<u8>,
    pub stats: Stats,
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
    untraced_sites: usize,
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
                untraced_sites: 0,
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

    /// Lowers every function and assembles the module.
    pub fn finish(mut self, options: &relax_session::Options, link: &dyn Link) -> Output {
        let opts = self.opts.clone();
        // user function types first (their indices are needed by every call)
        let mut bodies: Vec<Body> = Vec::with_capacity(self.funcs.len());
        let mut failed = Vec::new();
        let funcs = std::mem::take(&mut self.funcs);
        for (id, f) in &funcs {
            let r = lower_func(&opts, options, f, &self.fn_ix, &funcs, link, &mut self.sh);
            match r {
                Ok(b) => bodies.push(b),
                Err(e) => {
                    failed.push((*id, e));
                    let ty = func_type(f).map(|t| self.sh.ty(t)).unwrap_or_else(|_| self.sh.ty(FnType { params: vec![], results: vec![] }));
                    bodies.push(Body { bytes: vec![0, 0x00, 0x0B], fix: vec![], ty });
                }
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
            if let Some(&u) = self.fn_ix.get(id) {
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
            let key = |g: usize| if g < ni + n_user { funcs[g - ni].1.name.as_str() } else { "\u{10FFFF}" };
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
        let mut s = vec![1, 0x00];
        uleb(&mut s, pages);
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
        // globals: 0 = shadow stack pointer, 1 = heap base (immutable)
        let mut s = vec![2];
        s.extend_from_slice(&[0x7F, 1, 0x41]);
        sleb(&mut s, stack_top as i64);
        s.push(0x0B);
        s.extend_from_slice(&[0x7F, 0, 0x41]);
        sleb(&mut s, heap_base as i64);
        s.push(0x0B);
        let b0 = out.len();
        section(&mut out, 6, &s);
        sz(&mut st, &out, b0, 6);
        // exports: memory "m", heap base "h", tag "t", then the functions
        let mut s = Vec::new();
        let mut n = 2u64 + self.exports.len() as u64;
        if tag_ty.is_some() && opts.export_tag {
            n += 1;
        }
        uleb(&mut s, n);
        name(&mut s, "m");
        s.extend_from_slice(&[2, 0]);
        name(&mut s, "h");
        s.extend_from_slice(&[3, 1]);
        if tag_ty.is_some() && opts.export_tag {
            name(&mut s, "t");
            s.extend_from_slice(&[4, 0]);
        }
        for (nm, id) in &self.exports {
            name(&mut s, nm);
            s.push(0);
            let u = self.fn_ix[id];
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
            s.extend_from_slice(&fb);
        }
        let b0 = out.len();
        section(&mut out, 10, &s);
        sz(&mut st, &out, b0, 10);
        if !segs.is_empty() {
            let mut s = Vec::new();
            uleb(&mut s, segs.len() as u64);
            for d in &segs {
                match d.addr {
                    Some(a) => {
                        s.push(0);
                        s.push(0x41);
                        sleb(&mut s, a as u32 as i32 as i64);
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
        st.untraced_sites = self.sh.untraced_sites;
        // a folded copy emits no body: 0 bytes
        st.fn_sizes = funcs.iter().zip(bodies.iter()).enumerate().map(|(k, ((id, f), b))| (*id, f.name.clone(), if canon[ni + k] == ni + k { b.bytes.len() } else { 0 })).collect();
        Output { bytes: out, stats: st }
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
    last_br0: usize,
    last_transfer: usize,
    last_ret: usize,
    /// (end, opcode position, local, vreg) of the last `local.set`
    last_set: (usize, usize, u32, u32),
    sp0: u32,
}

fn lower_func(o: &Options, options: &relax_session::Options, f0: &Func, fn_ix: &HashMap<u32, u32>, funcs: &[(u32, &Func)], link: &dyn Link, sh: &mut Shared) -> Result<Body, String> {
    // vector ops wasm has no instruction for: per-lane scalar code; switches: br_table
    let native = |op: VX| vx_native(op);
    let pruned: Option<Func> = prune(o, f0, fn_ix, link);
    let f0: &Func = pruned.as_ref().unwrap_or(f0);
    let legal: Option<Func> = crate::legal::needed(f0, &native, true).then(|| {
        let mut g = f0.clone();
        crate::legal::legalize(&mut g, &native, true);
        g
    });
    let f0: &Func = legal.as_ref().unwrap_or(f0);
    let switched = crate::switch::lower_switches(options, f0);
    let f: &Func = switched.as_ref().map(|s| &s.0).unwrap_or(f0);
    let ft = func_type(f)?;
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
        last_br0: usize::MAX,
        last_transfer: usize::MAX,
        last_ret: usize::MAX,
        last_set: (usize::MAX, 0, 0, 0),
        sp0: NONE,
    };
    fx.check()?;
    fx.plan_folds();
    let (slot_at, end) = crate::frame::slot_layout(f, 0, true);
    let frame = ((end + 15) & !15) as u32;
    fx.slot_at = slot_at;
    fx.frame = frame;
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
        fx.out.push(I32_SUB);
        fx.out.push(LTEE);
        let fp = fx.fp;
        uleb(&mut fx.out, fp as u64);
        fx.out.extend_from_slice(&[GSET, 0]);
    } else if has_invoke {
        fx.out.extend_from_slice(&[GGET, 0, LSET]);
        let fp = fx.fp;
        uleb(&mut fx.out, fp as u64);
    }
    if fx.dispatch {
        fx.emit_dispatch()?;
    } else {
        fx.do_tree(0)?;
    }
    if !ft.results.is_empty() && fx.last_transfer != fx.out.len() {
        fx.out.push(0x00);
    }
    // a trailing `return` is the function's end
    if fx.last_ret == fx.out.len() && fx.ctx.is_empty() {
        fx.out.pop();
    }
    fx.out.push(END);
    body.extend_from_slice(&fx.out);
    let fix = fx.fix.iter().map(|(p, x)| (p + hdr, *x)).collect();
    Ok(Body { bytes: body, fix, ty })
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
        Inst::FBin(_, _, d, _, _) | Inst::FUnary(_, _, d, _) | Inst::FCmp(_, _, d, _, _) | Inst::Conv(_, d, _) | Inst::Load(_, d, _, _) | Inst::LoadX(_, d, _, _, _, _) | Inst::SlotAddr(d, _) | Inst::Addr(d, _) | Inst::FnAddr(d, _) | Inst::TlsAddr(d, _) => Some(*d),
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
                let direct = match c {
                    Callee::Fn(id) | Callee::CFn(id, _) => self.fn_ix.contains_key(id),
                    Callee::Host(s) | Callee::CHost(s, _) => !matches!(self.link.host(s.id), HostBind::Trap),
                    Callee::Indirect(_) | Callee::CIndirect(..) => true,
                    Callee::Asm(_) => false,
                };
                if !direct {
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
                    Inst::Addr(d, s) | Inst::TlsAddr(d, s) if self.o.remat && small(self.link.sym(s.id).wrapping_add(s.off as u64) as i64) => *d,
                    Inst::SlotAddr(d, _) if self.o.remat_slots && self.uses[d.0 as usize] <= 2 * base_uses[d.0 as usize] + 1 => *d,
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
        let add_edge = |adj: &mut Vec<Vec<u32>>, a: u32, b: u32| {
            if a != b && f.vregs[a as usize] == f.vregs[b as usize] {
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
            ctype.push(val_of(f.vregs[p.0 as usize])?);
            cuse.push(ucount[p.0 as usize] as u64);
        }
        let mut order: Vec<u32> = (0..nv as u32).filter(|v| touched[*v as usize] && colour[*v as usize] == NONE).collect();
        order.sort_by(|a, b| ucount[*b as usize].cmp(&ucount[*a as usize]).then(a.cmp(b)));
        let mut busy: Vec<bool> = Vec::new();
        for v in order {
            let t = val_of(f.vregs[v as usize])?;
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
        let aty = Val::I32;
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
    fn aconst(&mut self, v: i64) {
        self.i32c(v as i32)
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
        self.op(BR);
        self.u(d as u64);
        if d == 0 {
            self.last_br0 = self.out.len();
        }
        self.transfer();
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
            return self.from_nat(want);
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

    /// Converts a natural i64 on the stack to `want`.
    fn from_nat(&mut self, want: Want) -> Result<u64, String> {
        match want {
            Want::Nat => Ok(0),
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
                    self.op(WRAP);
                    Ok(0)
                } else {
                    self.op(WRAP);
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
        let inst = self.f.blocks[b as usize].insts[i].clone();
        let last = i + 1 == self.f.blocks[b as usize].insts.len();
        let d64 = |f: &Self, v: VReg| f.f.vregs[v.0 as usize] == Cls::F64;
        match &inst {
            Inst::Iconst(_, v) => {
                if let Want::Addr(off) = want {
                    let a = v.wrapping_add(off);
                    if (0..=u32::MAX as i64).contains(&a) {
                        self.aconst(a as u32 as i32 as i64);
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
                self.value(b, *a, Want::Nat)?;
                self.value(b, *x, Want::Nat)?;
                self.op(icmp_op(c, *s));
                self.cmp_result(want)
            }
            Inst::ICmpI(c, s, _, a, imm) => {
                let c = if want == Want::Inverted { inv(*c) } else { *c };
                self.value(b, *a, Want::Nat)?;
                if *imm == 0 && c == Cond::Eq {
                    self.op(I64_EQZ);
                } else {
                    self.i64c(*imm);
                    self.op(icmp_op(c, *s));
                }
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
                    // the f32 product is exact in f64; one more rounding of the sum
                    self.value(b, *a, Want::Nat)?;
                    self.op(F64_PROMOTE);
                    if *np {
                        self.op(0x9A);
                    }
                    self.value(b, *x, Want::Nat)?;
                    self.op(F64_PROMOTE);
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
                self.op(EXT_U);
                if so != 0 {
                    self.i64c(so);
                    self.op(I64_ADD);
                }
                self.from_nat(want)
            }
            Inst::Addr(_, s) | Inst::TlsAddr(_, s) => {
                let v = self.sym_val(*s);
                if let Want::Addr(off) = want {
                    let a = v.wrapping_add(off);
                    if (0..=u32::MAX as i64).contains(&a) {
                        self.aconst(a as u32 as i32 as i64);
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
                self.value(b, *s, Want::Nat)?;
                if i {
                    self.op(WRAP);
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
                self.value(b, *s, Want::Nat)?;
                if i {
                    self.op(WRAP);
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
                        self.value(b, *x, Want::Nat)?;
                        if *w == VW::D {
                            self.simd(18);
                        } else {
                            self.op(WRAP);
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
                        self.value(b, *y, Want::Nat)?;
                        match w {
                            VW::D => self.simd(30),
                            _ => {
                                self.op(WRAP);
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
            Want::Truthy | Want::Inverted => Ok(0),
            _ => {
                self.op(EXT_U);
                self.from_nat(want)
            }
        }
    }

    fn ibin(&mut self, b: u32, op: IOp, it: IntTy, a: VReg, x: Option<VReg>, imm: i64) -> Result<(), String> {
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
                        return Err(format!("call of slot {} with a signature unlike its body's", id));
                    }
                    push_args(self)?;
                    self.call(FRef::User(u));
                }
                None => {
                    self.untraced(*id);
                    return Ok(());
                }
            },
            Callee::Host(s) | Callee::CHost(s, _) => match self.link.host(s.id) {
                HostBind::Import(m, f) => {
                    let t = sig(self)?;
                    let ty = self.sh.ty(t);
                    let i = self.sh.import(&m, &f, ty);
                    push_args(self)?;
                    self.call(FRef::Import(i));
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
                self.value(b, *v, Want::Nat)?;
                self.op(WRAP);
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
        self.sh.untraced_sites += 1;
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
            for r in rets.iter().rev() {
                self.set(*r);
            }
            return Ok(());
        }
        if let Some(d) = tree_def(&inst) {
            if self.uses[d.0 as usize] == 0 && pure(&inst) {
                return Ok(());
            }
            if let Inst::Mov(d, a) = &inst {
                if self.fold_def[a.0 as usize].0 == NONE && self.loc[d.0 as usize] == self.loc[a.0 as usize] {
                    return Ok(());
                }
            }
            self.expr(b, i, Want::Nat)?;
            self.set(d);
            return Ok(());
        }
        match &inst {
            Inst::Store(m, base, off, src) => {
                let o = self.value(b, *base, Want::Addr(*off as i64))?;
                self.value(b, *src, Want::Nat)?;
                let (op, al) = self.store_op(*m)?;
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
                self.value(b, *src, Want::Nat)?;
                let (op, al) = self.store_op(*m)?;
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
                if *n > 0 {
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
            self.op(I32_ADD);
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
                self.stmt(b, i)?;
            }
        }
        self.term(b)
    }

    fn restore_sp(&mut self) {
        if self.sp0 != NONE {
            self.lget(self.sp0);
            self.out.extend_from_slice(&[GSET, 0]);
        } else if self.frame > 0 {
            self.lget(self.fp);
            self.aconst(self.frame as i64);
            self.op(I32_ADD);
            self.out.extend_from_slice(&[GSET, 0]);
        }
    }

    fn term(&mut self, b: u32) -> Result<(), String> {
        let t = self.f.blocks[b as usize].term.clone();
        match &t {
            Term::Jump(x) => self.do_branch(b, *x)?,
            Term::Branch(c, x, y) => self.branch(b, *c, *x, *y)?,
            Term::Ret(vs) => {
                for v in vs {
                    self.value(b, *v, Want::Nat)?;
                }
                self.restore_sp();
                self.op(RETURN);
                self.transfer();
                self.last_ret = self.out.len();
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
                self.op(ELSE);
                self.do_tree(second)?;
                self.ctx.pop();
                self.op(END);
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
        if tab.min == 0 && !tree {
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
        self.op(0x54);
        self.op(SELECT);
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
            // p = a * b
            g(&mut c, 0);
            g(&mut c, 1);
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
            let aw = |c: &mut Vec<u8>, l: u8| {
                c.push(LGET);
                c.push(l);
                c.push(WRAP);
            };
            match bi {
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
                    c.push(I32C);
                    sleb(&mut c, kb);
                    c.push(I32C);
                    sleb(&mut c, kb);
                    c.extend_from_slice(&[0x29, 3, 0, I64C, 1, I64_ADD, LTEE, l, 0x37, 3, 0]);
                    g(&mut c, l);
                    if t.results.first() == Some(&I32) {
                        c.push(WRAP);
                    }
                }
                Builtin::KeyGet | Builtin::KeySet => {
                    let kb = sh.keys_base;
                    g(&mut c, 0);
                    if t.params[0] == I64 {
                        c.push(WRAP);
                    }
                    c.extend_from_slice(&[I32C, 3, 0x74]);
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
                    locals.push((1, I32));
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
    Body { bytes: body, fix: fix.into_iter().map(|(p, x)| (p + hdr, x)).collect(), ty }
}
