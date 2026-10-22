//! Native code through the shared back end (relax-backend): AIR is lowered to RIR, run
//! through the tier-0 passes and emitted by the shared x64 (or arm64) code generator --
//! the same back end as Relax's Rust JIT (one vector IR for kernels, std::simd and core::arch).
//!
//! AIR's total semantics are spelled out in RIR, op by op, so the result is bit-identical
//! to [`crate::ir::run`]:
//! - i32 values live zero-extended (as u32) in 64-bit registers, so +, -, *, &, |, ^, <<, >>>
//!   are single 32-bit instructions (x64 32-bit forms and AArch64 W forms zero-extend for
//!   free); signed compares, >>, division and int->float conversions sign-extend first.
//! - `a / b` is `sel(b == 0, 0, a / sel(b == 0, 1, b))`, `a % b` keeps `a` for b == 0;
//!   i32::MIN / -1 cannot trap (the divide is 64-bit) and re-extends to i32::MIN.
//! - shift amounts are masked to 31; `>>>` shifts the zero-extended value.
//! - f32 -> i32 is RIR's saturating conversion (NaN -> 0); min/max are the select forms
//!   (`a < b ? a : b`); round is half away from zero; the only fusion is AIR's explicit Fma.
//! - memory offsets clamp as unsigned compares + selects (skipped where [`crate::ir::bounds`]
//!   proves the offset in range); host buffers clamp `base + off` (64-bit) to their length.
//!
//! Entry ABI (the same as compute's arm64 backend): `extern "C" fn(ctx, state, shared,
//! io, n)` (`extern "sysv64"` on x86-64, on every OS), io = [in_l, in_r, out_l, out_r] for audio programs and the (pointer, length)
//! buffer table for kernels.
use makepad_relax_api::relax_api;

use crate::ir::{Bin, Block, Cmp, Fma, Op, Program, Region, SliceArg, Stmt, Ty, Un, Val};
use relax_backend::exec;
use relax_backend::rir::{self, Cls, Cond, Conv, FOp, FUn, Func, IOp, Inst, IntTy, Mem, Term, VReg};

#[cfg(target_arch = "x86_64")]
use relax_backend::x64 as target;
#[cfg(target_arch = "aarch64")]
use relax_backend::arm64 as target;

const I32: IntTy = IntTy { bits: 32, signed: true };
const U32: IntTy = IntTy { bits: 32, signed: false };
const I64: IntTy = rir::I64;
const U64: IntTy = rir::U64;
/// i32 words load zero-extended: AIR integers live as zero-extended u32 in 64-bit registers
const W32: Mem = Mem::Int(4, false);
const PTR: Mem = Mem::Int(8, false);

/// The emitted code's entry: the System V ABI on x86-64 on every OS (relax_backend::x64
/// emits it; Windows' "C" ABI passes in rcx/rdx/r8/r9 and the kernel read garbage), the
/// platform C ABI on arm64.
#[cfg(target_arch = "x86_64")]
type Entry = unsafe extern "sysv64" fn(*mut u32, *mut u32, *mut u32, *const *mut f32, u32);
#[cfg(not(target_arch = "x86_64"))]
type Entry = unsafe extern "C" fn(*mut u32, *mut u32, *mut u32, *const *mut f32, u32);

/// Native code for one AIR program (shared back end).
#[relax_api(plain)]
pub struct Code {
    code: exec::Code,
}

impl Code {
    pub fn len(&self) -> usize {
        self.code.len()
    }

    pub fn is_empty(&self) -> bool {
        self.code.is_empty()
    }

    /// The machine code bytes.
    pub fn bytes(&self) -> &[u8] {
        self.code.bytes()
    }

    /// The code as 32-bit words (AArch64 instructions; disassembly).
    pub fn words(&self) -> &[u32] {
        let b = self.code.bytes();
        // SAFETY: the mapping is page-aligned and holds `len` bytes.
        unsafe { std::slice::from_raw_parts(b.as_ptr() as *const u32, b.len() / 4) }
    }

    /// Runs an audio program (see `arm64::Code::run`).
    ///
    /// # Safety
    /// As `arm64::Code::run`: slice sizes as compiled for, n >= 1, `shared` alive.
    pub unsafe fn run(&self, ctx: &mut [u32], state: &mut [u32], shared: *mut u32, ins: [&[f32]; 2], outs: [&mut [f32]; 2], n: u32) {
        let [o0, o1] = outs;
        let io = [ins[0].as_ptr() as *mut f32, ins[1].as_ptr() as *mut f32, o0.as_mut_ptr(), o1.as_mut_ptr()];
        unsafe {
            let f: Entry = std::mem::transmute(self.code.ptr());
            f(ctx.as_mut_ptr(), state.as_mut_ptr(), shared, io.as_ptr(), n);
        }
    }

    /// Runs a kernel over its (pointer, length) buffer table.
    ///
    /// # Safety
    /// As `arm64::Code::run_kernel`.
    pub unsafe fn run_kernel(&self, ctx: *mut u32, state: *mut u32, shared: *mut u32, table: *const u64, n: u32) {
        unsafe {
            let f: Entry = std::mem::transmute(self.code.ptr());
            f(ctx, state, shared, table as *const *mut f32, n);
        }
    }
}

/// Can this CPU run what the back end emits for `p` (x64: SSE4.1 rounding, FMA3 for Fma)?
fn cpu_ok(p: &Program) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        if !std::is_x86_feature_detected!("sse4.1") {
            return false;
        }
        if has_fma(&p.body) && !std::is_x86_feature_detected!("fma") {
            return false;
        }
    }
    let _ = p;
    true
}

/// The highest host buffer index a program accesses (-1: none).
pub(crate) fn max_buf(b: &Block) -> i32 {
    let mut m = -1;
    for s in b {
        let k = match s {
            Stmt::Def(_, Op::Load { region: Region::Buf(k), .. }) | Stmt::Store { region: Region::Buf(k), .. } => *k as i32,
            Stmt::If(_, t, e) => max_buf(t).max(max_buf(e)),
            Stmt::Loop { body, .. } => max_buf(body),
            _ => -1,
        };
        m = m.max(k);
    }
    m
}

#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
fn has_fma(b: &Block) -> bool {
    b.iter().any(|s| match s {
        Stmt::Def(_, Op::Fma(k, ..)) => *k != Fma::MulAddI,
        Stmt::If(_, t, e) => has_fma(t) || has_fma(e),
        Stmt::Loop { body, .. } => has_fma(body),
        _ => false,
    })
}

/// Compiles an audio program or a table `init`.
pub fn compile(p: &Program) -> Option<Code> {
    compile_with(p, 0)
}

/// Compiles a kernel whose host buffers have write permission `writable` (bit k: buffer
/// k), passed on to host calls.
pub fn compile_with(p: &Program, writable: u64) -> Option<Code> {
    let flat = crate::ir::flat(p);
    let p = &*flat;
    if !cpu_ok(p) {
        return None;
    }
    let key = crate::kcache::key("scalar", p, writable);
    if let Some(code) = key.and_then(load) {
        if crate::kcache::verify() {
            // verify mode: the stored code must be what a compile makes now
            let fresh = compile_fresh(p, writable, None).expect("a stored kernel compiles");
            crate::kcache::same("scalar", code.bytes(), fresh.bytes());
        }
        return Some(code);
    }
    compile_fresh(p, writable, key)
}

fn compile_fresh(p: &Program, writable: u64, key: Option<u128>) -> Option<Code> {
    let mut f = lower(p, writable);
    if f.verify().is_err() {
        return None;
    }
    optimise_for_codegen(&mut f);
    if f.verify().is_err() {
        return None;
    }
    emit_keyed(&f, key, true)
}

/// The stored machine code under `key`, re-targeted at this process's host trampoline (the
/// pool words `decode` checked lie inside the code).
pub(crate) fn load(key: u128) -> Option<Code> {
    if options().no_codegen {
        return None;
    }
    let (mut code, refs) = crate::kcache::decode(&(crate::kcache::hook()?.get)(key)?)?;
    let addr = crate::kcache::host_addr().to_le_bytes();
    for r in refs {
        code[r as usize..r as usize + 8].copy_from_slice(&addr);
    }
    Some(Code { code: exec::Code::new(&code)? })
}

static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The back end's options, read once per process from the environment (`MAKEPAD_RUN_OPT`,
/// `MAKEPAD_RUN_NO_AVX`, ...), with the host CPU's features. Kernels allocate registers by
/// weighted references over the square root of the live range's length (unless
/// MAKEPAD_RUN_RA_PRIO says otherwise): by references alone (Relax's default) a kernel's
/// long-lived values took every register and its short temporaries -- a noise hash, x64's 12
/// vector registers -- went through spill slots (x64 cycles per element: one-pass heightfield
/// x4 -23%, instances scalar -12%, x4 -13%; no kernel slower). On AArch64 the quick round
/// (four-wide kernels) coalesces copies too (`MAKEPAD_RUN_NO_COALESCE` turns it off): a loop
/// variable's update `x' = x * k; x = x'` and the fbm accumulator kept two registers and a copy
/// per iteration (instructions per element, x4 heights 211 -> 199, one pass 612 -> 580). Not on
/// x86-64: there it removed register moves the core eliminates anyway (heights x4 -3%
/// instructions) and measured +1.3% cycles on heights x4 (165, two tq tickets).
pub(crate) fn options() -> &'static relax_session::Options {
    static O: std::sync::OnceLock<relax_session::Options> = std::sync::OnceLock::new();
    O.get_or_init(|| {
        let mut o = relax_session::Options::from_env();
        if std::env::var_os("MAKEPAD_RUN_RA_PRIO").is_none() {
            o.ra_prio = relax_session::options::Priority::Sqrt;
        }
        o.ra_quick_coalesce = cfg!(target_arch = "aarch64");
        #[cfg(target_arch = "x86_64")]
        {
            o.cpu = relax_backend::x64::detect_cpu(&o);
        }
        o
    })
}

/// The symbol table host calls are linked through: `Callee::Host(Sym(HOST_TRAMPOLINE))`.
const HOST_TRAMPOLINE: u32 = 0;

/// The value of a relocated field of kernel code: only the host trampoline's address.
pub(crate) fn reloc_value(r: rir::Reloc) -> u64 {
    match r {
        rir::Reloc::Sym(s) if s.id == HOST_TRAMPOLINE => (crate::host::trampoline as *const () as usize as u64).wrapping_add(s.off as u64),
        rir::Reloc::Value(v) => v,
        _ => 0,
    }
}

/// Machine code for optimised RIR, in its own executable mapping, kept in the store under `key`
/// when there is one. `thorough`: the back end's thorough register allocation (coalescing, spill
/// windows, live-range splitting at loops: three allocation rounds); off, one quick round.
/// Scalar kernels take it (Stage-like kernels 2-9% faster on arm64). The four-wide kernels do not:
/// they are the large ones (Stage's largest: 66k instructions in 14k blocks), there it cost 27% of
/// Stage's cold-start instructions (x64: 732 -> 536 G, CPU at present -6%), and their throughput
/// is the same with the quick round (bench_kernels, arm64 mini and x64).
pub(crate) fn emit_keyed(f: &Func, key: Option<u128>, thorough: bool) -> Option<Code> {
    if options().no_codegen {
        // MAKEPAD_RUN_NO_CODEGEN: no executable memory; the interpreter runs the program
        return None;
    }
    let env = target::Env { slots: rir::SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: thorough, syms: 0, sym_class: 0, near: false };
    let mut c = target::compile(options(), f, &env).ok()?;
    if c.relocs.iter().any(|r| !matches!(r.target, rir::Reloc::Sym(rir::Sym { id: HOST_TRAMPOLINE, .. }) | rir::Reloc::Value(_))) {
        return None;
    }
    // the code is linked before it is mapped: a pool word (Far symbol) needs no base address,
    // and the kernel code has no other relocations
    rir::link(&mut c.code, 0, &c.relocs, &reloc_value).ok()?;
    // the store keeps code whose only relocations are the trampoline's pool words (the code is
    // otherwise position independent): `load` writes this process's address into them
    let host_refs: Option<Vec<u32>> = c
        .relocs
        .iter()
        .map(|r| matches!((r.kind, r.target), (rir::RelocKind::Pool64, rir::Reloc::Sym(rir::Sym { id: HOST_TRAMPOLINE, off: 0 }))).then_some(r.at))
        .collect();
    if let Ok(dir) = std::env::var("COMPUTE_NATIVE_DUMP") {
        // RIR and machine code of every compiled program (bring-up and perf work)
        let k = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::write(format!("{}/kernel{}.rir", dir, k), f.dump());
        let _ = std::fs::write(format!("{}/kernel{}.bin", dir, k), &c.code[..c.text_len as usize]);
    }
    let code = exec::Code::new(&c.code)?;
    if let (Some(key), Some(hook), Some(refs)) = (key, crate::kcache::hook(), host_refs) {
        (hook.put)(key, crate::kcache::encode(&c.code, &refs));
    }
    if let Ok(dir) = std::env::var("COMPUTE_NATIVE_DUMP") {
        let _ = std::fs::write(format!("{}/kernel{}.addr", dir, SEQ.load(std::sync::atomic::Ordering::Relaxed).wrapping_sub(1)), format!("{:#x}\n", code.ptr() as usize));
    }
    Some(Code { code })
}

/// The lowered and optimised RIR of a program (tests, dumps).
pub fn rir_of(p: &Program, writable: u64) -> Func {
    let flat = crate::ir::flat(p);
    let mut f = lower(&flat, writable);
    optimise_for_codegen(&mut f);
    f
}

/// The tier-0 passes as Relax's baseline runs them: folding, copy propagation, CSE, DCE, SROA
/// of the frame, LICM, block layout, address modes, loop rotation; after LICM, value numbering
/// of pure values (passes::gvn_pure). No fusion, no float reassociation. `MAKEPAD_RUN_OPT=none` (or `-fold`, `-cse`, `-sroa`, `-licm`, `-imm`,
/// `-layout`) turns passes off.
pub(crate) fn optimise_for_codegen(f: &mut Func) {
    use relax_backend::passes;
    let o = options();
    let none = o.opt.len() == 1 && o.opt[0] == "none";
    let on = |tok: &str| !none && !relax_backend::stats::opt_off(o, tok);
    let (fold, cse, sroa, licm, imm, layout, gvn) = (on("-fold"), on("-cse"), on("-sroa"), on("-licm"), on("-imm"), on("-layout"), on("-gvn"));
    if fold {
        passes::simplify(o, f, None, imm, cse);
        if sroa && passes::sroa(f) {
            // the cells' entry zeros that a write always precedes are dead by liveness only
            passes::live_dce(f);
            passes::simplify(o, f, None, imm, cse);
        }
        // LICM gathers the loop's invariant lane extracts in its preheader; local CSE's bounded
        // table leaves most repeats there (kernels have thousands), value numbering merges them
        let hoisted = licm && passes::licm(o, f);
        let numbered = cse && gvn && passes::gvn_pure(f);
        if hoisted || numbered {
            passes::simplify(o, f, None, imm, cse);
        }
        passes::sink_consts(f);
    }
    if layout {
        passes::relayout(f);
    }
    if fold {
        passes::addr_modes(f);
    }
    if layout {
        passes::rotate_loops(f);
    }
}

// =========================================================================
// AIR -> RIR
// =========================================================================

/// Appends instructions to block `cur` of `f` (shared by the scalar and vector lowerings).
pub(crate) struct Emitter<'f> {
    pub(crate) f: &'f mut Func,
    pub(crate) cur: u32,
}

impl Emitter<'_> {
    fn push(&mut self, i: Inst) {
        let b = &mut self.f.blocks[self.cur as usize];
        b.insts.push(i);
        b.pos.push(0);
    }
    fn k(&mut self, v: i64) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.push(Inst::Iconst(d, v));
        d
    }
    fn op(&mut self, o: IOp, it: IntTy, a: VReg, b: VReg) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.push(Inst::IBin(o, it, d, a, b));
        d
    }
    fn opk(&mut self, o: IOp, it: IntTy, a: VReg, k: i64) -> VReg {
        let c = self.k(k);
        self.op(o, it, a, c)
    }

    /// i32 `x / c` or `x % c` (as AIR: truncating, wrapping, x / 0 = 0, x % 0 = x) for a
    /// constant c, by shifts or the magic multiplier -- no divide instruction.
    pub(crate) fn div_const(&mut self, x: VReg, c: i32, rem: bool) -> VReg {
        match c {
            0 => return if rem { x } else { self.k(0) },
            1 => return if rem { self.k(0) } else { x },
            -1 => {
                if rem {
                    return self.k(0);
                }
                let d = self.f.vreg(Cls::I);
                self.push(Inst::INeg(I32, d, x));
                return d;
            }
            _ => {}
        }
        if c < 0 {
            let cv = self.k(c as i64);
            return self.op(if rem { IOp::Rem } else { IOp::Div }, I32, x, cv);
        }
        let q = if (c as u32).is_power_of_two() {
            // q = (x + (x < 0 ? c - 1 : 0)) >> k, in 64 bits (x is sign-extended)
            let k = (c as u32).trailing_zeros() as i64;
            let s = self.opk(IOp::Shr, I64, x, 63);
            let bias = self.opk(IOp::And, I64, s, c as i64 - 1);
            let t = self.op(IOp::Add, I64, x, bias);
            self.opk(IOp::Shr, I64, t, k)
        } else {
            // Hacker's Delight 10-1: t = hi32(x * m) [+ x] >> sh, q = t + (x >>> 31)
            let (m, sh) = crate::ir::magic_s32(c as u32);
            let p = self.opk(IOp::Mul, I64, x, m as i64);
            let mut t = self.opk(IOp::Shr, I64, p, 32);
            if m < 0 {
                t = self.op(IOp::Add, I32, t, x);
            }
            if sh > 0 {
                t = self.opk(IOp::Shr, I32, t, sh as i64);
            }
            let xu = self.f.vreg(Cls::I);
            self.push(Inst::Conv(Conv::IntToInt(U32), xu, x));
            let sb = self.opk(IOp::Shr, U64, xu, 31);
            self.op(IOp::Add, I32, t, sb)
        };
        if !rem {
            return q;
        }
        let qc = self.opk(IOp::Mul, I64, q, c as i64);
        self.op(IOp::Sub, I32, x, qc)
    }
}

struct Lx<'a> {
    p: &'a Program,
    f: Func,
    cur: u32,
    vals: Vec<VReg>,
    vars: Vec<VReg>,
    /// entry parameters: ctx, state, shared, io/table
    regions: [VReg; 4],
    /// n as an i32 value, n - 1 (as u64; clamps of audio I/O)
    n_u32: VReg,
    n_m1: VReg,
    frame: Option<u32>,
    /// (continue target, exit) per enclosing loop
    loops: Vec<(u32, u32)>,
    bounds: Vec<Option<u32>>,
    writable: u64,
    /// call record slot (host calls)
    rec: Option<u32>,
    /// constant values: materialised at each use (rematerialised, never kept live across
    /// loops: compute's AIR hoists every constant out of its loops)
    consts: Vec<Option<(u64, Cls)>>,
    /// the compare defining each bool value (re-emitted right before a select of it, so
    /// the back end fuses the two: flags + cmov, or a compare mask + blend)
    cmps: Vec<Option<Op>>,
    /// per host buffer: (base pointer, last word index), loaded once at entry (the table
    /// does not change during a call)
    bufs: Vec<(VReg, VReg)>,
}

fn cls_of(t: Ty) -> Cls {
    match t {
        Ty::F32 => Cls::F32,
        Ty::F64 => Cls::F64,
        Ty::I32 | Ty::Bool => Cls::I,
    }
}

fn cond(c: Cmp) -> Cond {
    match c {
        Cmp::Lt => Cond::Lt,
        Cmp::Le => Cond::Le,
        Cmp::Gt => Cond::Gt,
        Cmp::Ge => Cond::Ge,
        Cmp::Eq => Cond::Eq,
        Cmp::Ne => Cond::Ne,
    }
}

fn lower(p: &Program, writable: u64) -> Func {
    let mut f = Func::new("splash_kernel".to_string(), 0);
    let entry = f.block();
    let mut regions = [VReg(0); 4];
    for r in regions.iter_mut() {
        *r = f.vreg(Cls::I);
        f.params.push(*r);
    }
    let n_raw = f.vreg(Cls::I);
    f.params.push(n_raw);
    let vals: Vec<VReg> = p.vals.iter().map(|t| f.vreg(cls_of(*t))).collect();
    let vars: Vec<VReg> = p.vars.iter().map(|t| f.vreg(cls_of(*t))).collect();
    let frame = if p.frame_words > 0 { Some(f.slot(p.frame_words * 4, 16)) } else { None };
    let n_u32 = VReg(0);
    let n_m1 = VReg(0);
    let mut lx = Lx { p, f, cur: entry, vals, vars, regions, n_u32, n_m1, frame, loops: Vec::new(), bounds: crate::ir::bounds(p), writable, rec: None, consts: vec![None; p.vals.len()], cmps: vec![None; p.vals.len()], bufs: Vec::new() };
    // n arrives as a C u32 (upper bits undefined)
    let nu = lx.conv(Conv::IntToInt(U32), n_raw, Cls::I);
    lx.n_u32 = nu;
    lx.n_m1 = lx.ibini(IOp::Sub, I64, nu, 1);
    for k in 0..=max_buf(&p.body) {
        let table = lx.regions[3];
        let ptr = lx.f.vreg(Cls::I);
        lx.emit(Inst::Load(PTR, ptr, table, k as i32 * 16));
        let len = lx.f.vreg(Cls::I);
        lx.emit(Inst::Load(PTR, len, table, k as i32 * 16 + 8));
        let last = lx.ibini(IOp::Sub, I64, len, 1);
        lx.bufs.push((ptr, last));
    }
    lx.block(&p.body);
    lx.term(Term::Ret(Vec::new()));
    lx.f
}

impl<'a> Lx<'a> {
    fn emit(&mut self, i: Inst) {
        let b = &mut self.f.blocks[self.cur as usize];
        b.insts.push(i);
        b.pos.push(0);
    }

    fn term(&mut self, t: Term) {
        self.f.blocks[self.cur as usize].term = t;
    }

    fn new_block(&mut self) -> u32 {
        self.f.block()
    }

    fn v(&mut self, x: Val) -> VReg {
        match self.consts[x.0 as usize] {
            Some((bits, Cls::I)) => self.iconst(bits as i64),
            Some((bits, cls)) => {
                let d = self.f.vreg(cls);
                self.emit(Inst::Fconst(d, bits, cls == Cls::F64));
                d
            }
            None => self.vals[x.0 as usize],
        }
    }

    fn iconst(&mut self, k: i64) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::Iconst(d, k));
        d
    }

    fn ibin(&mut self, op: IOp, it: IntTy, a: VReg, b: VReg) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::IBin(op, it, d, a, b));
        d
    }

    fn ibini(&mut self, op: IOp, it: IntTy, a: VReg, k: i64) -> VReg {
        let c = self.iconst(k);
        self.ibin(op, it, a, c)
    }

    fn icmpi(&mut self, c: Cond, signed: bool, a: VReg, k: i64) -> VReg {
        let d = self.f.vreg(Cls::I);
        let kk = self.iconst(k);
        self.emit(Inst::ICmp(c, signed, d, a, kk));
        d
    }

    fn conv(&mut self, c: Conv, a: VReg, cls: Cls) -> VReg {
        let d = self.f.vreg(cls);
        self.emit(Inst::Conv(c, d, a));
        d
    }

    fn sel(&mut self, c: VReg, a: VReg, b: VReg) -> VReg {
        let d = self.f.vreg(self.f.vregs[a.0 as usize]);
        self.emit(Inst::Sel(d, c, a, b));
        d
    }

    // -- memory ------------------------------------------------------------

    /// (address vreg, byte offset) of a region word.
    fn addr(&mut self, region: Region, base: u32, extent: u32, off: Option<Val>) -> (VReg, i32) {
        if let Region::Buf(k) = region {
            let (ptr, lm1) = self.bufs[k as usize];
            // index = min(base + zext(off), len - 1), 64-bit
            let idx = match off {
                Some(o) => {
                    let zo = self.v(o);
                    if base > 0 {
                        self.ibini(IOp::Add, I64, zo, base as i64)
                    } else {
                        zo
                    }
                }
                None => self.iconst(base as i64),
            };
            let c = self.f.vreg(Cls::I);
            self.emit(Inst::ICmp(Cond::Le, false, c, idx, lm1));
            let idx = self.sel(c, idx, lm1);
            let sc = self.ibini(IOp::Shl, I64, idx, 2);
            let a = self.ibin(IOp::Add, I64, ptr, sc);
            return (a, 0);
        }
        let rb = match region {
            Region::Ctx => self.regions[0],
            Region::State => self.regions[1],
            Region::Shared => self.regions[2],
            Region::Frame => {
                let d = self.f.vreg(Cls::I);
                self.emit(Inst::SlotAddr(d, self.frame.unwrap_or(0)));
                d
            }
            Region::Buf(_) => unreachable!(),
        };
        let o = match off {
            None => return (rb, (base * 4) as i32),
            Some(o) => o,
        };
        let ov = self.v(o);
        let proven = self.bounds[o.0 as usize].is_some_and(|b| b < extent && b < 1 << 31);
        let idx = if proven {
            ov
        } else {
            let top = extent.saturating_sub(1) as i64;
            let c = self.icmpi(Cond::Le, false, ov, top);
            let t = self.iconst(top);
            self.sel(c, ov, t)
        };
        let sc = self.ibini(IOp::Shl, I64, idx, 2);
        let a = self.ibin(IOp::Add, I64, rb, sc);
        (a, (base * 4) as i32)
    }

    /// Clamped audio frame index `idx` into 0..n, scaled to bytes, plus channel pointer.
    fn io_addr(&mut self, slot: u32, idx: Val) -> VReg {
        let ptr = self.f.vreg(Cls::I);
        self.emit(Inst::Load(PTR, ptr, self.regions[3], slot as i32 * 8));
        let iv = self.v(idx);
        let c = self.f.vreg(Cls::I);
        self.emit(Inst::ICmp(Cond::Le, false, c, iv, self.n_m1));
        let i = self.sel(c, iv, self.n_m1);
        let sc = self.ibini(IOp::Shl, I64, i, 2);
        self.ibin(IOp::Add, I64, ptr, sc)
    }

    // -- statements --------------------------------------------------------

    fn block(&mut self, b: &Block) {
        for s in b {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Def(v, op) => self.def(*v, op),
            Stmt::Set(var, v) => {
                let (d, x) = (self.vars[var.0 as usize], self.v(*v));
                self.emit(Inst::Mov(d, x));
            }
            Stmt::Store { region, base, extent, off, val } => {
                let x = self.v(*val);
                let m = match self.p.vals[val.0 as usize] {
                    Ty::F32 => Mem::F32,
                    _ => W32,
                };
                let (a, o) = self.addr(*region, *base, *extent, *off);
                self.emit(Inst::Store(m, a, o, x));
            }
            Stmt::Out { ch, idx, val } => {
                let a = self.io_addr(2 + *ch as u32, *idx);
                let old = self.f.vreg(Cls::F32);
                self.emit(Inst::Load(Mem::F32, old, a, 0));
                let sum = self.f.vreg(Cls::F32);
                let x = self.v(*val);
                self.emit(Inst::FBin(FOp::Add, false, sum, old, x));
                self.emit(Inst::Store(Mem::F32, a, 0, sum));
            }
            Stmt::If(c, t, e) => {
                let (bt, be, join) = (self.new_block(), self.new_block(), self.new_block());
                let cv = self.v(*c);
                self.term(Term::Branch(cv, bt, be));
                self.cur = bt;
                self.block(t);
                self.term(Term::Jump(join));
                self.cur = be;
                self.block(e);
                self.term(Term::Jump(join));
                self.cur = join;
            }
            Stmt::Loop { cap, body } => {
                let exit = self.new_block();
                if *cap <= 1 {
                    // one iteration (inlined-function wrappers): continue and break both leave
                    let bb = self.new_block();
                    self.term(Term::Jump(bb));
                    self.cur = bb;
                    if *cap == 1 {
                        self.loops.push((exit, exit));
                        self.block(body);
                        self.loops.pop();
                    }
                    self.term(Term::Jump(exit));
                    self.cur = exit;
                    return;
                }
                let cnt = self.iconst(0);
                let head = self.new_block();
                let bb = self.new_block();
                self.term(Term::Jump(head));
                self.cur = head;
                let more = self.icmpi(Cond::Lt, false, cnt, *cap as i64);
                self.term(Term::Branch(more, bb, exit));
                self.cur = bb;
                let one = self.iconst(1);
                self.emit(Inst::IBin(IOp::Add, I64, cnt, cnt, one));
                self.loops.push((head, exit));
                self.block(body);
                self.loops.pop();
                self.term(Term::Jump(head));
                self.cur = exit;
            }
            Stmt::Break(d) => {
                let (_, exit) = self.loops[self.loops.len() - 1 - *d as usize];
                self.term(Term::Jump(exit));
                self.cur = self.new_block();
            }
            Stmt::Continue(d) => {
                let (head, _) = self.loops[self.loops.len() - 1 - *d as usize];
                self.term(Term::Jump(head));
                self.cur = self.new_block();
            }
            Stmt::CallHost { f, args, slices, rets } => self.call_host(*f, args, slices, rets),
            Stmt::Call { .. } => unreachable!("flattened"),
        }
    }

    /// A host call: the record in a frame slot, then `host::trampoline(record, table, ctx,
    /// writable)`, then the result read back from the record.
    fn call_host(&mut self, f: u16, args: &[Val], slices: &[SliceArg], rets: &[Val]) {
        let slot = match self.rec {
            Some(s) => s,
            None => {
                let s = self.f.slot(64 * 4, 16);
                self.rec = Some(s);
                s
            }
        };
        let rec = self.f.vreg(Cls::I);
        self.emit(Inst::SlotAddr(rec, slot));
        let put_int = |lx: &mut Self, at: usize, v: VReg| lx.emit(Inst::Store(W32, rec, 4 * at as i32, v));
        for (k, h) in [f as i64, args.len() as i64, slices.len() as i64, rets.len() as i64].into_iter().enumerate() {
            let c = self.iconst(h);
            put_int(self, k, c);
        }
        let mut at = crate::host::REC_HEADER;
        for a in args {
            let x = self.v(*a);
            if self.p.vals[a.0 as usize] == Ty::F32 {
                self.emit(Inst::Store(Mem::F32, rec, 4 * at as i32, x));
            } else {
                put_int(self, at, x);
            }
            at += 1;
        }
        for x in slices {
            let b = self.iconst(x.buf as i64);
            put_int(self, at, b);
            let o = self.v(x.off);
            put_int(self, at + 1, o);
            let l = self.v(x.len);
            put_int(self, at + 2, l);
            at += 3;
        }
        let w = self.iconst(self.writable as i64);
        self.emit(Inst::Call(rir::Callee::Host(rir::Sym::new(HOST_TRAMPOLINE)), vec![rec, self.regions[3], self.regions[0], w], Vec::new()));
        for v in rets {
            let d = self.v(*v);
            let m = if self.p.vals[v.0 as usize] == Ty::F32 { Mem::F32 } else { W32 };
            self.emit(Inst::Load(m, d, rec, 4 * at as i32));
        }
    }

    fn def(&mut self, v: Val, op: &Op) {
        let konst = match *op {
            Op::ConstF(x) => Some((x.to_bits() as u64, Cls::F32)),
            Op::ConstD(x) => Some((x.to_bits(), Cls::F64)),
            Op::ConstI(x) => Some((x as u32 as u64, Cls::I)),
            Op::ConstB(x) => Some((x as u64, Cls::I)),
            _ => None,
        };
        if konst.is_some() {
            self.consts[v.0 as usize] = konst;
            return;
        }
        let d = self.vals[v.0 as usize];
        if matches!(op, Op::CmpF(..) | Op::CmpD(..) | Op::CmpI(..)) {
            self.cmps[v.0 as usize] = Some(*op);
        }
        match *op {
            Op::ConstF(x) => self.emit(Inst::Fconst(d, x.to_bits() as u64, false)),
            Op::ConstD(x) => self.emit(Inst::Fconst(d, x.to_bits(), true)),
            Op::ConstI(x) => self.emit(Inst::Iconst(d, x as u32 as i64)),
            Op::ConstB(x) => self.emit(Inst::Iconst(d, x as i64)),
            Op::Get(var) => {
                let s = self.vars[var.0 as usize];
                self.emit(Inst::Mov(d, s));
            }
            Op::Un(u, a) => {
                let a = self.v(a);
                self.un(d, u, a)
            }
            Op::Bin(b, x, y) => {
                let (x, y) = (self.v(x), self.v(y));
                self.bin(d, b, x, y)
            }
            Op::CmpF(c, x, y) => {
                let (x, y) = (self.v(x), self.v(y));
                self.emit(Inst::FCmp(cond(c), false, d, x, y))
            }
            Op::CmpD(c, x, y) => {
                let (x, y) = (self.v(x), self.v(y));
                self.emit(Inst::FCmp(cond(c), true, d, x, y))
            }
            Op::CmpI(c, x, y) => {
                let (x, y) = (self.v(x), self.v(y));
                let (x, y) = self.signed_pair(c, x, y);
                self.emit(Inst::ICmp(cond(c), true, d, x, y))
            }
            Op::Sel(c, x, y) => {
                let (xv, yv) = (self.v(x), self.v(y));
                let cv = match self.cmps[c.0 as usize] {
                    Some(Op::CmpF(cc, a, b)) | Some(Op::CmpD(cc, a, b)) | Some(Op::CmpI(cc, a, b)) => {
                        let w = matches!(self.cmps[c.0 as usize], Some(Op::CmpD(..)));
                        let (av, bv) = (self.v(a), self.v(b));
                        let t = self.f.vreg(Cls::I);
                        match self.cmps[c.0 as usize] {
                            Some(Op::CmpI(..)) => {
                                let (av, bv) = self.signed_pair(cc, av, bv);
                                self.emit(Inst::ICmp(cond(cc), true, t, av, bv))
                            }
                            _ => self.emit(Inst::FCmp(cond(cc), w, t, av, bv)),
                        }
                        t
                    }
                    _ => self.v(c),
                };
                self.emit(Inst::Sel(d, cv, xv, yv))
            }
            Op::Fma(Fma::MulAddI, a, b, c) => {
                let (a, b, c) = (self.v(a), self.v(b), self.v(c));
                let m = self.ibin(IOp::Mul, U32, a, b);
                self.emit(Inst::IBin(IOp::Add, U32, d, m, c));
            }
            Op::Fma(k, a, b, c) => {
                let (np, na) = match k {
                    Fma::Add => (false, false),
                    Fma::SubFrom => (true, false),
                    _ => (false, true),
                };
                let (a, b, c) = (self.v(a), self.v(b), self.v(c));
                self.emit(Inst::FMulAdd(np, na, false, d, a, b, c));
            }
            Op::Wrap(x, len) => {
                let xv = self.v(x);
                if len.is_power_of_two() {
                    let r = self.ibini(IOp::And, U32, xv, (len - 1) as i64);
                    self.emit(Inst::Mov(d, r));
                } else {
                    // (x as i32) % (len as i32), negative remainders + len (as eval_wrap)
                    let l = len as i32 as i64;
                    let sx = self.conv(Conv::IntToInt(I32), xv, Cls::I);
                    let cur = self.cur;
                    let r = Emitter { f: &mut self.f, cur }.div_const(sx, len as i32, true);
                    let neg = self.icmpi(Cond::Lt, true, r, 0);
                    let up = self.ibini(IOp::Add, I32, r, l);
                    let w = self.sel(neg, up, r);
                    self.emit(Inst::Conv(Conv::IntToInt(U32), d, w));
                }
            }
            Op::Load { region, base, extent, off } => {
                let m = match self.p.vals[v.0 as usize] {
                    Ty::F32 => Mem::F32,
                    _ => W32,
                };
                let (a, o) = self.addr(region, base, extent, off);
                self.emit(Inst::Load(m, d, a, o));
            }
            Op::In { ch, idx } => {
                let a = self.io_addr(ch as u32, idx);
                self.emit(Inst::Load(Mem::F32, d, a, 0));
            }
            Op::FrameCount => self.emit(Inst::Mov(d, self.n_u32)),
            Op::BufLen(k) => {
                // the low word of the length, as an i32
                self.emit(Inst::Load(W32, d, self.regions[3], k as i32 * 16 + 8));
            }
        }
    }

    fn un(&mut self, d: VReg, u: Un, a: VReg) {
        let fun = |op: FUn, w: bool| Inst::FUnary(op, w, d, a);
        let i = match u {
            Un::NegF => fun(FUn::Neg, false),
            Un::AbsF => fun(FUn::Abs, false),
            Un::SqrtF => fun(FUn::Sqrt, false),
            Un::FloorF => fun(FUn::Floor, false),
            Un::CeilF => fun(FUn::Ceil, false),
            Un::TruncF => fun(FUn::Trunc, false),
            Un::RoundF => fun(FUn::RoundAway, false),
            Un::NegD => fun(FUn::Neg, true),
            Un::AbsD => fun(FUn::Abs, true),
            Un::SqrtD => fun(FUn::Sqrt, true),
            Un::FloorD => fun(FUn::Floor, true),
            Un::CeilD => fun(FUn::Ceil, true),
            Un::TruncD => fun(FUn::Trunc, true),
            Un::RoundD => fun(FUn::RoundAway, true),
            Un::F2I | Un::D2I => {
                let r = self.conv(Conv::FToInt(u == Un::D2I, I32), a, Cls::I);
                Inst::Conv(Conv::IntToInt(U32), d, r)
            }
            Un::I2F | Un::I2D => {
                let sx = self.conv(Conv::IntToInt(I32), a, Cls::I);
                Inst::Conv(Conv::IntToF(I32, u == Un::I2D), d, sx)
            }
            Un::F2D => Inst::Conv(Conv::F32ToF64, d, a),
            Un::D2F => Inst::Conv(Conv::F64ToF32, d, a),
            Un::BitsIF => Inst::Conv(Conv::BitsToF(false), d, a),
            Un::BitsFI => Inst::Conv(Conv::FToBits(false), d, a),
            Un::LoD => {
                let b = self.conv(Conv::FToBits(true), a, Cls::I);
                Inst::Conv(Conv::IntToInt(U32), d, b)
            }
            Un::HiD => {
                let b = self.conv(Conv::FToBits(true), a, Cls::I);
                let k = self.iconst(32);
                Inst::IBin(IOp::Shr, U64, d, b, k)
            }
            Un::NegI => Inst::INeg(U32, d, a),
            Un::NotB => {
                let z = self.iconst(0);
                Inst::ICmp(Cond::Eq, true, d, a, z)
            }
        };
        self.emit(i);
    }

    /// Operands of an i32 compare: ordered compares need them sign-extended (values are
    /// held zero-extended); equality compares take them as they are.
    fn signed_pair(&mut self, c: Cmp, x: VReg, y: VReg) -> (VReg, VReg) {
        if matches!(c, Cmp::Eq | Cmp::Ne) {
            return (x, y);
        }
        let sx = self.conv(Conv::IntToInt(I32), x, Cls::I);
        let sy = self.conv(Conv::IntToInt(I32), y, Cls::I);
        (sx, sy)
    }

    /// The i32 constant a vreg was just set to (constants are materialised at each use).
    fn const_of(&self, v: VReg) -> Option<i32> {
        for i in self.f.blocks[self.cur as usize].insts.iter().rev() {
            if let Inst::Iconst(d, k) = i {
                if *d == v {
                    return Some(*k as i32);
                }
            }
        }
        None
    }

    fn bin(&mut self, d: VReg, b: Bin, x: VReg, y: VReg) {
        let fb = |op: FOp, w: bool| Inst::FBin(op, w, d, x, y);
        let i = match b {
            Bin::AddF => fb(FOp::Add, false),
            Bin::SubF => fb(FOp::Sub, false),
            Bin::MulF => fb(FOp::Mul, false),
            Bin::DivF => fb(FOp::Div, false),
            Bin::MinF => fb(FOp::MinLt, false),
            Bin::MaxF => fb(FOp::MaxGt, false),
            Bin::AddD => fb(FOp::Add, true),
            Bin::SubD => fb(FOp::Sub, true),
            Bin::MulD => fb(FOp::Mul, true),
            Bin::DivD => fb(FOp::Div, true),
            Bin::MinD => fb(FOp::MinLt, true),
            Bin::MaxD => fb(FOp::MaxGt, true),
            Bin::AddI => Inst::IBin(IOp::Add, U32, d, x, y),
            Bin::SubI => Inst::IBin(IOp::Sub, U32, d, x, y),
            Bin::MulI => Inst::IBin(IOp::Mul, U32, d, x, y),
            Bin::AndI | Bin::AndB => Inst::IBin(IOp::And, U32, d, x, y),
            Bin::OrI | Bin::OrB => Inst::IBin(IOp::Or, U32, d, x, y),
            Bin::XorI => Inst::IBin(IOp::Xor, U32, d, x, y),
            Bin::DivI | Bin::RemI if self.const_of(y).is_some() => {
                let c = self.const_of(y).unwrap();
                let sx = self.conv(Conv::IntToInt(I32), x, Cls::I);
                let cur = self.cur;
                let r = Emitter { f: &mut self.f, cur }.div_const(sx, c, b == Bin::RemI);
                Inst::Conv(Conv::IntToInt(U32), d, r)
            }
            Bin::DivI | Bin::RemI => {
                let sx = self.conv(Conv::IntToInt(I32), x, Cls::I);
                let sy = self.conv(Conv::IntToInt(I32), y, Cls::I);
                let z = self.icmpi(Cond::Eq, true, sy, 0);
                let one = self.iconst(1);
                let safe = self.sel(z, one, sy);
                let op = if b == Bin::DivI { IOp::Div } else { IOp::Rem };
                let q = self.ibin(op, I32, sx, safe);
                let qu = self.conv(Conv::IntToInt(U32), q, Cls::I);
                let zero_case = if b == Bin::DivI { self.iconst(0) } else { x };
                Inst::Sel(d, z, zero_case, qu)
            }
            Bin::ShlI | Bin::ShrUI => {
                let k = self.ibini(IOp::And, U32, y, 31);
                Inst::IBin(if b == Bin::ShlI { IOp::Shl } else { IOp::Shr }, U32, d, x, k)
            }
            Bin::ShrI => {
                let k = self.ibini(IOp::And, U32, y, 31);
                let sx = self.conv(Conv::IntToInt(I32), x, Cls::I);
                let r = self.ibin(IOp::Shr, I32, sx, k);
                Inst::Conv(Conv::IntToInt(U32), d, r)
            }
            Bin::MakeD => {
                let hi = self.ibini(IOp::Shl, I64, x, 32);
                let bits = self.ibin(IOp::Or, U64, hi, y);
                Inst::Conv(Conv::BitsToF(true), d, bits)
            }
        };
        self.emit(i);
    }
}
