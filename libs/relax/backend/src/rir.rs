//! RIR: Relax's low-level IR. A CFG of blocks over typed virtual registers
//! (mutable, not SSA, like wasm locals), explicit memory, explicit traps.
//! Every instruction carries a source position for the PC -> source map.

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Cls {
    /// 64-bit integer / pointer register
    I,
    F32,
    F64,
    /// 128-bit vector of four 32-bit lanes (f32 or i32 by op); lives in float registers
    V128,
    /// 256-bit vector (x64 AVX ymm): made only by `pair256` after optimisation, from V128
    /// pairs that are the two halves of one value; every op on it is the V128 op on both halves
    V256,
}

impl Cls {
    /// a vector class (lives in a float register, saved and spilled whole)
    pub fn is_vec(self) -> bool {
        matches!(self, Cls::V128 | Cls::V256)
    }
    /// bytes of a spill slot of this class (vector slots 16-aligned)
    pub fn spill_bytes(self) -> u32 {
        match self {
            Cls::V128 => 16,
            Cls::V256 => 32,
            _ => 8,
        }
    }
}

/// Lane width of a lane-typed vector op (`Inst::VX`): 8, 16, 32 or 64 bits (float ops: S = f32,
/// D = f64).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum VW {
    B,
    H,
    S,
    D,
}

impl VW {
    pub fn bits(self) -> u32 {
        match self {
            VW::B => 8,
            VW::H => 16,
            VW::S => 32,
            VW::D => 64,
        }
    }
}

/// Which NaN a float vector op produces when an input is NaN (the other bits are IEEE-exact).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum NanBits {
    /// the host instruction's NaN (Rust leaves the payload unspecified)
    Host,
    /// SSE: the first NaN operand, quieted; the negative default NaN otherwise
    Intel,
    /// NEON: signaling NaNs first; the positive default NaN otherwise
    Neon,
}

/// Rounding of `VX::FRound`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RoundTo {
    Floor,
    Ceil,
    Trunc,
    /// half away from zero
    Away,
}

/// Lane-typed 128-bit vector ops (`Inst::VX(op, d, a, b)`): the core::arch intrinsics'
/// operations (NEON / SSE2..SSE4.1 semantics; each backend picks its instructions).
/// Unary ops ignore `b` (it repeats `a`). `d` is a V128 unless noted; scalar operands and
/// results are I vregs (lanes zero-extended).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum VX {
    // ---- binary, V128 x V128 -> V128, lane-wise
    /// float lanes (S: f32, D: f64)
    FAdd(VW, NanBits),
    FSub(VW, NanBits),
    FMul(VW, NanBits),
    FDiv(VW, NanBits),
    /// `a < b ? a : b` per lane
    FMinLt(VW),
    /// `a > b ? a : b` per lane
    FMaxGt(VW),
    /// NEON propagating min/max, including signed-zero ordering
    FMinNeon(VW),
    FMaxNeon(VW),
    /// all ones / zero per lane, the scalar compare's meaning (`Ne` is true for unordered lanes)
    FCmp(Cond, VW),
    /// signed integer compare, all ones / zero per lane
    ICmp(Cond, VW),
    /// shifts by per-lane amounts (each in 0..lane bits)
    ShlV(VW),
    ShrSV(VW),
    ShrUV(VW),
    /// unsigned integer compare, all ones / zero per lane
    UCmp(Cond, VW),
    /// IEEE minNum / maxNum: a NaN lane takes the other operand (both NaN: a quiet NaN); the
    /// sign of a zero result is unspecified (Rust's `simd_minimum_number_nsz`)
    FMinNum(VW),
    FMaxNum(VW),
    /// lanes of a then b truncated to half their width (wrapping; lane = the source width)
    Narrow(VW),
    /// the two f64 lanes of a then of b rounded to f32 (four f32 lanes)
    FCvtNarrow,
    Add(VW),
    Sub(VW),
    /// low half of the product (H, S)
    MulLo(VW),
    MinS(VW),
    MaxS(VW),
    MinU(VW),
    MaxU(VW),
    /// all ones / zero per lane
    CmpEq(VW),
    CmpGtS(VW),
    AddSatU(VW),
    SubSatU(VW),
    AddSatS(VW),
    SubSatS(VW),
    /// (a + b + 1) >> 1 unsigned (B, H)
    AvgU(VW),
    And,
    Or,
    Xor,
    /// a & !b
    AndNot,
    /// interleave the low halves: a0 b0 a1 b1 ... (unpacklo / zip1)
    ZipLo(VW),
    /// interleave the high halves (unpackhi / zip2)
    ZipHi(VW),
    /// i16 x i16 products, adjacent pairs summed into i32 (pmaddwd)
    MaddH,
    /// u8 (a) x i8 (b) products, adjacent pairs summed with i16 saturation (pmaddubsw)
    MaddUbs,
    /// per 8-byte half: sum of |a - b| over u8 lanes, as u64 (psadbw)
    SadU8,
    /// i16 lanes of a then b -> u8 with unsigned saturation (packuswb)
    PackUsH,
    /// i16 lanes of a then b -> i8 with signed saturation (packsswb)
    PackSsH,
    /// i32 lanes of a then b -> i16 with signed saturation (packssdw)
    PackSsS,
    /// i32 lanes of a then b -> u16 with unsigned saturation (packusdw)
    PackUsS,
    /// d[i] = b[i] & 0x80 ? 0 : a[b[i] & 15] (pshufb)
    ByteShuffle,
    // SHA instruction semantics are architecture-specific (not interchangeable lane orders).
    X86Sha1Rnds4(u8),
    X86Sha1NextE,
    X86Sha1Msg1,
    X86Sha1Msg2,
    X86Sha256Msg1,
    X86Sha256Msg2,
    ArmSha1Su1,
    ArmSha256Su0,
    // Unary: SHA1H reads/writes lane 0, with the rest of the result zero.
    ArmSha1H,
    // Ternary: Inst::VX3, all operands/results V128; SHA1C/P/M read b.s[0].
    X86Sha256Rnds2,
    ArmSha1C,
    ArmSha1P,
    ArmSha1M,
    ArmSha1Su0,
    ArmSha256H,
    ArmSha256H2,
    ArmSha256Su1,
    /// shift by the signed amount in each lane (NEON USHL / SSHL): left by n >= 0, right by -n
    /// (logical / arithmetic) for n < 0, |n| below the lane bits. Made by the arm64 back end only
    /// (a right shift by `y + c` is `ushl` by `-c - y`: one `sub`, no `neg`).
    ArmUShl(VW),
    ArmSShl(VW),
    // ---- unary, V128 -> V128
    FNeg(VW),
    FAbs(VW),
    FSqrt(VW, NanBits),
    FRound(VW, RoundTo),
    /// float -> integer of the same width, truncating, saturating, NaN -> 0 (Rust `as`)
    FToI(VW),
    /// signed integer -> float of the same width, round to nearest
    IToF(VW),
    /// wrapping integer negation
    Neg(VW),
    /// f32 lanes 0 and 1 widened to two f64 lanes (exact)
    FCvtLo,
    Abs(VW),
    ShlI(VW, u8),
    ShrUI(VW, u8),
    ShrSI(VW, u8),
    /// 32-bit lane shuffle by an 8-bit selector (pshufd)
    ShufS(u8),
    /// whole-register byte shift right / left, zero fill (psrldq / pslldq)
    BytesShrI(u8),
    BytesShlI(u8),
    /// low / high half widened to 2x lane width, zero / sign extended (lane = source width)
    WidenLoU(VW),
    WidenHiU(VW),
    WidenLoS(VW),
    WidenHiS(VW),
    /// adjacent pairs added into 2x-wide lanes, unsigned (uaddlp; lane = source width)
    PairAddLongU(VW),
    // ---- scalar <-> vector
    /// I -> V128: every lane = the scalar's low bits
    Splat(VW),
    /// V128 -> I: lane `k`, zero-extended
    Extract(VW, u8),
    /// (V128 a, I b) -> V128: a with lane `k` = b
    Insert(VW, u8),
    /// V128 -> I: the top bit of each byte (pmovmskb)
    MoveMaskB,
    /// V128 -> I: the top bit of each lane of width w, lane i at bit i
    MoveMask(VW),
    /// V128 -> I: wrapping sum of all lanes, zero-extended from the lane width
    AddAcross(VW),
    /// V128 -> I: unsigned maximum of all lanes
    MaxAcrossU(VW),
    /// I -> V128: the scalar in the low 64 bits, upper bits zero
    FromLo64,
    // ---- 256-bit halves (made only by `pair256`)
    /// (V128 lo, V128 hi) -> V256
    Join,
    /// V256 -> V128: the low (false) or high (true) half
    Half(bool),
}

impl VX {
    /// Operands read: 1 (unary, scalar-from), 2, or 3 (Inst::VX3).
    pub fn arity(self) -> usize {
        match self {
            VX::ArmSha1H | VX::FCvtLo | VX::MoveMask(_) | VX::FNeg(_) | VX::FAbs(_) | VX::FSqrt(..) | VX::FRound(..) | VX::FToI(_) | VX::IToF(_) | VX::Neg(_) | VX::Abs(_) | VX::ShlI(..) | VX::ShrUI(..) | VX::ShrSI(..) | VX::ShufS(_) | VX::BytesShrI(_) | VX::BytesShlI(_) | VX::WidenLoU(_) | VX::WidenHiU(_) | VX::WidenLoS(_) | VX::WidenHiS(_) | VX::PairAddLongU(_) | VX::Splat(_) | VX::Extract(..) | VX::MoveMaskB | VX::AddAcross(_) | VX::MaxAcrossU(_) | VX::FromLo64 | VX::Half(_) => 1,
            VX::X86Sha256Rnds2 | VX::ArmSha1C | VX::ArmSha1P | VX::ArmSha1M
            | VX::ArmSha1Su0 | VX::ArmSha256H | VX::ArmSha256H2 | VX::ArmSha256Su1 => 3,
            _ => 2,
        }
    }
    /// the result is an I vreg
    /// Extract, MoveMaskB, AddAcross, MaxAcrossU: an I result, the lane zero-extended to 64 bits
    pub fn scalar_result(self) -> bool {
        matches!(self, VX::Extract(..) | VX::MoveMaskB | VX::MoveMask(_) | VX::AddAcross(_) | VX::MaxAcrossU(_))
    }
}

/// Bit operations on a 64-bit integer (`IBit`): counts are 0..=64 (clz/ctz of 0 = 64);
/// narrower types are masked/shifted around them by the front end.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BitOp {
    Popcnt,
    Clz,
    Ctz,
    Bswap,
    /// bit reverse
    Rev,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct VReg(pub u32);

/// Integer type of an operation: width in bits and signedness. Narrow values are
/// kept extended to 64 bits in registers (sign- or zero-extended by type).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct IntTy {
    pub bits: u8,
    pub signed: bool,
}

pub const I64: IntTy = IntTy { bits: 64, signed: true };
pub const U64: IntTy = IntTy { bits: 64, signed: false };
pub const I32: IntTy = IntTy { bits: 32, signed: true };
pub const U32: IntTy = IntTy { bits: 32, signed: false };

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum IOp {
    Add,
    Sub,
    Mul,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Div,
    Rem,
    /// high 64 bits of the 128-bit product of the 64-bit operands (signed by IntTy.signed);
    /// IntTy.bits is 64
    MulHi,
    /// rotate left by the second operand (mod the width); made by the optimiser from
    /// `x << k | x >> (w - k)` for unsigned 32-bit and 64-bit values only
    Rotl,
    /// `a & ((1 << (b mod 64)) - 1)`: the low `b mod 64` bits of a (IntTy is U64); made by the
    /// optimiser from that mask idiom (bit readers' `bitbuf & bitmask(n)`): x64 BMI2 `bzhi`,
    /// arm64 `lsl` of all ones + `bic`
    Bzhi,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Cond {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FOp {
    Add,
    Sub,
    Mul,
    Div,
    /// Rust `f64::min`/`max`: if one operand is NaN the other is returned (IEEE minNum)
    Min,
    Max,
    /// `a < b ? a : b` (a NaN or equal operands give b: minss/fcmp+fcsel; Splash kernels)
    MinLt,
    /// `a > b ? a : b`
    MaxGt,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FUn {
    Neg,
    Abs,
    Sqrt,
    Floor,
    Ceil,
    Trunc,
    RoundEven,
    /// round half away from zero (Rust `round`)
    RoundAway,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Conv {
    /// re-extend an integer to a (narrower or wider) integer type
    IntToInt(IntTy),
    IntToF(IntTy, bool),   // from, to f64?
    FToInt(bool, IntTy),   // from f64?, to (saturating, NaN -> 0)
    F32ToF64,
    F64ToF32,
    BitsToF(bool),
    FToBits(bool),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Mem {
    /// integer load/store of `bytes` (load extends by signedness)
    Int(u8, bool),
    F32,
    F64,
}

/// A value the embedder resolves at link time: entry `id` of its symbol value table (`Env::syms`,
/// u64 per entry) plus `off`. Code never embeds the value as an immediate, only through a
/// relocation, so the embedder can move code between runs (Relax's disk cache) by rewriting the
/// relocated words; freshly compiled code already holds the current values.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct Sym {
    pub id: u32,
    pub off: i64,
}

impl Sym {
    pub fn new(id: u32) -> Sym {
        Sym { id, off: 0 }
    }
    pub fn plus(self, k: i64) -> Sym {
        Sym { id: self.id, off: self.off.wrapping_add(k) }
    }
    /// The value from a symbol table (`table` = address of the embedder's u64 array).
    ///
    /// # Safety
    /// `table` must point to at least `id + 1` readable u64s.
    pub unsafe fn value(self, table: u64) -> u64 {
        std::ptr::read((table as *const u64).add(self.id as usize)).wrapping_add(self.off as u64)
    }
}

/// How a backend may encode a symbol's value (`Env::sym_class`, one byte per `Sym::id`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SymClass {
    /// an address inside the embedder's code/data reservation: pc-relative
    Near = 0,
    /// an address anywhere (host or C symbols): an 8-byte pool word
    Far = 1,
    /// a per-run number, not an address (fits in 32 bits): an immediate field
    Imm = 2,
    /// an offset into the running thread's domain static block (fits in 32 bits): `Inst::Addr`
    /// of it is the thread's domain base (slot 0 of its thread-local block) + the symbol's value
    Domain = 3,
}

impl SymClass {
    pub fn of(table: u64, id: u32) -> SymClass {
        if table == 0 {
            return SymClass::Far;
        }
        match unsafe { std::ptr::read((table as *const u8).add(id as usize)) } {
            0 => SymClass::Near,
            2 => SymClass::Imm,
            3 => SymClass::Domain,
            _ => SymClass::Far,
        }
    }
}

/// How one relocated field of a function's code is encoded (`RelocAt::kind`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RelocKind {
    /// the 8-byte word at `at` = value (Far symbols; cold glue targets)
    Pool64,
    /// x64: the disp32 at `at`, which ends its instruction: value - (base + at + 4)
    X64Rel32,
    /// arm64 adrp at `at`: page(value) - page(base + at), in pages
    A64Adrp,
    /// arm64 add (immediate) at `at`: value & 0xfff
    A64AddLo12,
    /// arm64 ldr x (unsigned offset, scale 8) at `at`: (value & 0xfff) / 8 (value 8-aligned)
    A64LdrLo12X,
    /// the 4 bytes at `at` = value (must fit in 32 bits)
    Imm32,
    /// arm64 movz at `at` and movk (lsl 16) at `at + 4`: the low 32 bits of value (must fit)
    A64MovzMovk,
    /// x64: the disp32 of a `call`/`jmp` at `at` (ends the instruction): value - (base + at + 4);
    /// a target out of reach is branched to through a veneer (`link_with`)
    X64Branch32,
    /// arm64 `b`/`bl` at `at`: (value - pc) / 4 in imm26 (+-128 MB); out of reach: a veneer
    A64Branch26,
}

/// Reach of a pc-relative branch field from the branch's own pc (`RelocKind::X64Branch32`: from
/// the end of the disp32, `A64Branch26`: from the instruction).
pub fn branch_reaches(kind: RelocKind, pc: u64, target: u64) -> bool {
    let d = target.wrapping_sub(pc) as i64;
    match kind {
        RelocKind::X64Branch32 => d >= i32::MIN as i64 && d <= i32::MAX as i64,
        RelocKind::A64Branch26 => d & 3 == 0 && (-(1i64 << 27)..1i64 << 27).contains(&d),
        _ => false,
    }
}

/// One relocated field: where, how, and what value goes there.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RelocAt {
    pub at: u32,
    pub kind: RelocKind,
    pub target: Reloc,
}

/// Where the function slots live. A slot (u32 id) owns a thunk (its permanent callable
/// address), a lazy stub, a table word and a tier record; they sit in segments that are
/// reserved when the ids first reach them, so the number of functions has no fixed bound
/// (ids are u32; the rest is address space). Segment 0 holds ids `0..2^18`, segment k >= 1 the
/// ids `2^(17+k)..2^(18+k)`: doubling, so 4G ids take 15 reservations. Nothing needs the
/// segments to be near each other or near the code: code reaches a slot through a full 64-bit
/// address (a literal in its pool), thunks and stubs hold absolute addresses.
pub struct SlotTable {
    /// per kind (thunk, stub, table, tier), per segment: the segment's first byte (0 = none yet)
    bases: [[std::sync::atomic::AtomicU64; SLOT_SEGS]; 4],
    /// bytes per slot of each kind
    stride: [u64; 4],
    /// the embedder's hook for the address C code gets for a slot (`callable`): 0 = none
    callable_hook: std::sync::atomic::AtomicUsize,
    callable_ctx: std::sync::atomic::AtomicUsize,
}

pub const SLOT_SEGS: usize = 15;
const SK_THUNK: usize = 0;
const SK_STUB: usize = 1;
const SK_TABLE: usize = 2;
const SK_TIER: usize = 3;

impl SlotTable {
    pub fn new(thunk: u64, stub: u64, table: u64, tier: u64) -> Self {
        SlotTable {
            bases: [const { [const { std::sync::atomic::AtomicU64::new(0) }; SLOT_SEGS] }; 4],
            stride: [thunk, stub, table, tier],
            callable_hook: std::sync::atomic::AtomicUsize::new(0),
            callable_ctx: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Slots in segment `seg`.
    pub fn seg_slots(seg: usize) -> usize {
        if seg == 0 { 1 << 18 } else { 1 << (17 + seg) }
    }

    /// (segment, index in it) of a slot id.
    pub fn locate(id: u32) -> (usize, usize) {
        if id < 1 << 18 {
            (0, id as usize)
        } else {
            let bits = 32 - id.leading_zeros() as usize;
            (bits - 18, (id - (1u32 << (bits - 1))) as usize)
        }
    }

    /// First id of a segment.
    pub fn seg_first(seg: usize) -> u64 {
        if seg == 0 { 0 } else { 1 << (17 + seg) }
    }

    pub fn has_seg(&self, seg: usize) -> bool {
        self.bases[SK_TABLE][seg].load(std::sync::atomic::Ordering::Acquire) != 0
    }

    /// Records a segment's four arrays (published last: a set table base means all are set).
    pub fn install(&self, seg: usize, thunk: u64, stub: u64, table: u64, tier: u64) {
        use std::sync::atomic::Ordering::*;
        self.bases[SK_THUNK][seg].store(thunk, Relaxed);
        self.bases[SK_STUB][seg].store(stub, Relaxed);
        self.bases[SK_TIER][seg].store(tier, Relaxed);
        self.bases[SK_TABLE][seg].store(table, Release);
    }

    /// The first byte of each installed segment's thunk array.
    pub fn seg_base(&self, kind: usize, seg: usize) -> u64 {
        self.bases[kind][seg].load(std::sync::atomic::Ordering::Acquire)
    }

    fn addr(&self, kind: usize, id: u32) -> u64 {
        let (seg, ix) = Self::locate(id);
        self.bases[kind][seg].load(std::sync::atomic::Ordering::Acquire) + ix as u64 * self.stride[kind]
    }
    pub fn thunk(&self, id: u32) -> u64 { self.addr(SK_THUNK, id) }
    /// The embedder decides what address a slot has as a value (a function pointer): `hook(ctx,
    /// id, thunk)` answers it. Android: a C-ABI function handed to a system library is called
    /// through a stub inside the host library, as those libraries check indirect calls
    /// (CFI) against the code they know.
    pub fn set_callable_hook(&self, hook: fn(usize, u32, u64) -> u64, ctx: usize) {
        self.callable_ctx.store(ctx, std::sync::atomic::Ordering::Relaxed);
        self.callable_hook.store(hook as usize, std::sync::atomic::Ordering::Release);
    }
    /// The address of the slot as a value: its thunk, unless the embedder's hook says otherwise.
    pub fn callable(&self, id: u32) -> u64 {
        let thunk = self.thunk(id);
        let h = self.callable_hook.load(std::sync::atomic::Ordering::Acquire);
        if h == 0 {
            return thunk;
        }
        let hook: fn(usize, u32, u64) -> u64 = unsafe { std::mem::transmute(h) };
        hook(self.callable_ctx.load(std::sync::atomic::Ordering::Relaxed), id, thunk)
    }
    pub fn stub(&self, id: u32) -> u64 { self.addr(SK_STUB, id) }
    /// The function table word of a slot.
    pub fn table(&self, id: u32) -> u64 { self.addr(SK_TABLE, id) }
    pub fn tier(&self, id: u32) -> u64 { self.addr(SK_TIER, id) }

    /// The id whose entry of `kind` (0 thunk, 1 stub, 2 table, 3 tier) starts at or contains
    /// `addr`, with the offset into the entry.
    pub fn find(&self, kind: usize, addr: u64) -> Option<(u32, u64)> {
        for seg in 0..SLOT_SEGS {
            let base = self.bases[kind][seg].load(std::sync::atomic::Ordering::Acquire);
            if base == 0 {
                continue;
            }
            let len = Self::seg_slots(seg) as u64 * self.stride[kind];
            if addr >= base && addr < base + len {
                let k = addr - base;
                return Some(((Self::seg_first(seg) + k / self.stride[kind]) as u32, k % self.stride[kind]));
            }
        }
        None
    }
}

/// The unit's slot table as the backends' `Env` holds it (the unit owns the table and outlives
/// every compile that uses the env).
#[derive(Clone, Copy)]
pub struct SlotsPtr(pub *const SlotTable);
unsafe impl Send for SlotsPtr {}
unsafe impl Sync for SlotsPtr {}

impl SlotsPtr {
    /// no table (backend tests that never relocate a slot)
    pub const NONE: SlotsPtr = SlotsPtr(std::ptr::null());
    pub fn table(&self, id: u32) -> u64 {
        if self.0.is_null() { 0 } else { unsafe { (*self.0).table(id) } }
    }
    pub fn thunk(&self, id: u32) -> u64 {
        if self.0.is_null() { 0 } else { unsafe { (*self.0).thunk(id) } }
    }
    /// The slot's address as a value (SlotTable::callable).
    pub fn callable(&self, id: u32) -> u64 {
        if self.0.is_null() { 0 } else { unsafe { (*self.0).callable(id) } }
    }
}

/// Writes every relocated field of `code`, to run at address `base`. Backends emit the fields
/// zeroed, so emitted code is independent of where and in which run it runs; the embedder
/// links it here (fresh and cached code alike). Err: a value does not fit its field at `base`
/// (a branch field out of reach included: `link_with` takes veneers for those).
pub fn link(code: &mut [u8], base: u64, relocs: &[RelocAt], value: &dyn Fn(Reloc) -> u64) -> Result<(), String> {
    link_with(code, base, relocs, value, &|_, _, _| None)
}

/// `link`, where a branch field (`X64Branch32`, `A64Branch26`) whose target is out of reach
/// branches to `veneer(kind, target, pc)` instead: code in reach of `pc` that jumps to `target`
/// (None: no veneer, the link fails).
pub fn link_with(code: &mut [u8], base: u64, relocs: &[RelocAt], value: &dyn Fn(Reloc) -> u64, veneer: &dyn Fn(RelocKind, u64, u64) -> Option<u64>) -> Result<(), String> {
    for r in relocs {
        let at = r.at as usize;
        let mut v = value(r.target);
        if matches!(r.kind, RelocKind::X64Branch32 | RelocKind::A64Branch26) {
            let pc = base + at as u64 + if r.kind == RelocKind::X64Branch32 { 4 } else { 0 };
            if !branch_reaches(r.kind, pc, v) {
                v = match veneer(r.kind, v, pc) {
                    Some(t) if branch_reaches(r.kind, pc, t) => t,
                    _ => return Err(format!("branch out of reach ({:#x} from {:#x}) and no veneer", v, pc)),
                };
            }
        }
        let rd32 = |c: &[u8], o: usize| -> Result<u32, String> {
            c.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| format!("relocation at {} past the code", o))
        };
        let wr32 = |c: &mut [u8], o: usize, w: u32| c[o..o + 4].copy_from_slice(&w.to_le_bytes());
        match r.kind {
            RelocKind::Pool64 => {
                let w = code.get_mut(at..at + 8).ok_or_else(|| format!("relocation at {} past the code", at))?;
                w.copy_from_slice(&v.to_le_bytes());
            }
            RelocKind::X64Branch32 => {
                rd32(code, at)?;
                wr32(code, at, v.wrapping_sub(base + at as u64 + 4) as i64 as i32 as u32);
            }
            RelocKind::A64Branch26 => {
                let d = (v.wrapping_sub(base + at as u64) as i64 >> 2) as u32;
                let w = rd32(code, at)? & !0x03FF_FFFF;
                wr32(code, at, w | (d & 0x03FF_FFFF));
            }
            RelocKind::X64Rel32 => {
                let d = v.wrapping_sub(base + at as u64 + 4) as i64;
                if d < i32::MIN as i64 || d > i32::MAX as i64 {
                    return Err(format!("rel32 out of range ({:#x} from {:#x})", v, base + at as u64));
                }
                rd32(code, at)?;
                wr32(code, at, d as i32 as u32);
            }
            RelocKind::A64Adrp => {
                let pc = base + at as u64;
                let d = ((v & !0xfff) as i64).wrapping_sub((pc & !0xfff) as i64) >> 12;
                if d < -(1 << 20) || d >= 1 << 20 {
                    return Err(format!("adrp out of range ({:#x} from {:#x})", v, pc));
                }
                let w = rd32(code, at)? & !((3 << 29) | (0x7FFFF << 5));
                let d = d as u32;
                wr32(code, at, w | (d & 3) << 29 | ((d >> 2) & 0x7FFFF) << 5);
            }
            RelocKind::A64AddLo12 => {
                let w = rd32(code, at)? & !(0xFFF << 10);
                wr32(code, at, w | ((v & 0xfff) as u32) << 10);
            }
            RelocKind::A64LdrLo12X => {
                if v & 7 != 0 {
                    return Err(format!("ldr lo12 of an unaligned address {:#x}", v));
                }
                let w = rd32(code, at)? & !(0xFFF << 10);
                wr32(code, at, w | (((v & 0xfff) >> 3) as u32) << 10);
            }
            RelocKind::Imm32 => {
                if v > u32::MAX as u64 {
                    return Err(format!("imm32 value {:#x} does not fit", v));
                }
                rd32(code, at)?;
                wr32(code, at, v as u32);
            }
            RelocKind::A64MovzMovk => {
                if v > u32::MAX as u64 {
                    return Err(format!("movz/movk value {:#x} does not fit 32 bits", v));
                }
                let lo = rd32(code, at)? & !(0xFFFF << 5);
                let hi = rd32(code, at + 4)? & !(0xFFFF << 5);
                wr32(code, at, lo | ((v & 0xffff) as u32) << 5);
                wr32(code, at + 4, hi | (((v >> 16) & 0xffff) as u32) << 5);
            }
        }
    }
    Ok(())
}

/// The target of a relocated field (`RelocAt::target`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Reloc {
    /// a symbol's value + offset
    Sym(Sym),
    /// the function table word of a slot (`SlotTable::table`)
    Slot(u32),
    /// a slot's permanent callable address (`SlotTable::callable`): a value (fn pointers, vtables)
    Thunk(u32),
    /// a slot as a call target: its thunk (`SlotTable::thunk`), which jumps to the slot's current
    /// code (a direct branch the runtime keeps in step with the table word)
    Call(u32),
    /// the thread-local block key / cell offset (`Env::tls_key`, pre-scaled as the backend loads it)
    TlsKey,
    /// `Env::tls_slow`
    TlsSlow,
    /// `Env::resume`
    Resume,
    /// a value resolved by the embedder (linking code from a cache); never emitted by a backend
    Value(u64),
}

#[derive(Clone, PartialEq, Hash, Debug)]
pub enum Callee {
    /// through the function table slot (live mode) / direct (AOT)
    Fn(u32),
    Indirect(VReg),
    /// runtime function at a symbol's address (scalar arguments and results only)
    Host(Sym),
    /// foreign (C ABI) function at a symbol's address
    CHost(Sym, Box<CSig>),
    /// call through an `extern "C" fn` pointer
    CIndirect(VReg, Box<CSig>),
    /// direct call of a Relax `extern "C" fn` (function table slot)
    CFn(u32, Box<CSig>),
    /// an inline assembly block (`asm!`; `--no-asm` blocks it): the call's arguments are its
    /// inputs and its results its outputs, each in the block's register. Every pass sees it as a
    /// call (side effects, memory, no motion); the register allocator keeps all other values out
    /// of its registers over it (`AsmBlock::regs`); the backend emits the bytes in place.
    Asm(Box<AsmBlock>),
}

/// The options of an inline assembly block (Reference [asm.options]) that the backend reads.
pub const ASM_NORETURN: u32 = 1;
pub const ASM_NOSTACK: u32 = 2;
pub const ASM_NOMEM: u32 = 4;
pub const ASM_READONLY: u32 = 8;
pub const ASM_PURE: u32 = 16;
pub const ASM_PRESERVES_FLAGS: u32 = 32;

/// An `asm!` block assembled at lowering (`asm` module): its machine code, with its operands
/// in fixed registers.
#[derive(Clone, PartialEq, Hash, Debug)]
pub struct AsmBlock {
    pub code: Vec<u8>,
    /// fields that hold a symbol's value: (offset in `code`, encoding, symbol)
    pub fixups: Vec<(u32, crate::asm::FixKind, Sym)>,
    /// the register of each input (the call's argument vregs, in order)
    pub ins: Vec<crate::asm::PReg>,
    /// the register of each output (the call's result vregs, in order)
    pub outs: Vec<crate::asm::PReg>,
    /// every register the block reads or may write (`PReg::bit`): inputs, outputs, discarded
    /// outputs, clobbers. No other value is held in them over the block.
    pub regs: u64,
    /// the registers it may write: callee-saved ones among them are saved by the prologue
    pub writes: u64,
    /// ASM_* bits
    pub opts: u32,
}

/// An aggregate passed or returned by value under the C ABI.
#[derive(Clone, PartialEq, Hash, Debug)]
pub struct CAgg {
    pub size: u32,
    pub align: u32,
    /// scalar fields in offset order (nested structs and arrays flattened); empty when
    /// unknown or very large (the aggregate is then classified as memory)
    pub fields: Vec<(u32, Mem)>,
}

#[derive(Clone, PartialEq, Hash, Debug)]
pub enum CArg {
    /// the vreg holds the value (integers extended to 64 bits as everywhere in RIR)
    Scalar(Mem),
    /// the vreg holds the address of a private copy of the aggregate
    Agg(CAgg),
}

/// C-family calling convention, separate from ABI identity and unwinding.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u8)]
pub enum CConv { Host, SysV64, Win64 }

impl CConv {
    pub fn target(self) -> crate::cabi::Target {
        match self {
            Self::Host => crate::cabi::HOST,
            Self::SysV64 => crate::cabi::Target::SysV,
            Self::Win64 => crate::cabi::Target::Win64,
        }
    }
}

/// C-family ABI shape of a call or function body; the backend lowers it.
/// ABI argument vregs: [result address if `ret` is Some] then one per `args` entry.
/// With an aggregate return the result is written through that address and the call
/// has no result vregs; scalar results come back in the call's result vregs as usual.
#[derive(Clone, PartialEq, Hash, Debug)]
pub struct CSig {
    pub conv: CConv,
    pub args: Vec<CArg>,
    pub ret: Option<CAgg>,
    /// variadic: the number of fixed arguments in `args`; u32::MAX when not variadic
    pub n_fixed: u32,
    /// a narrow integer result (i8..i32, u8..u32, bool): C leaves the register's upper bits
    /// undefined, so the backend re-extends it after the call
    pub ret_narrow: Option<IntTy>,
    /// the callee's `-unwind` ABI lets a panic unwind through the call (rustc FnAbi::can_unwind):
    /// the call has an unwind edge, as a Rust call does. Other C-family ABIs abort a panic
    /// leaving their body.
    pub unwinds: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AtomOrd {
    Relaxed,
    Acquire,
    Release,
    AcqRel,
    SeqCst,
    /// a volatile access (`read_volatile` / `write_volatile`): an ordinary load or store
    /// instruction that no pass may remove, merge, forward or reorder with other volatile or
    /// atomic accesses -- the guarantees every AtomicLoad / AtomicStore already has
    Volatile,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RmwOp {
    Xchg,
    Add,
    Sub,
    And,
    Nand,
    Or,
    Xor,
    Max,
    Min,
    UMax,
    UMin,
}

#[derive(Clone, PartialEq, Hash, Debug)]
pub enum Inst {
    Iconst(VReg, i64),
    Fconst(VReg, u64, bool),
    Mov(VReg, VReg),
    IBin(IOp, IntTy, VReg, VReg, VReg),
    /// `d = a op imm` (imm fits in i32)
    IBinI(IOp, IntTy, VReg, VReg, i64),
    INeg(IntTy, VReg, VReg),
    INot(IntTy, VReg, VReg),
    /// 64-bit bit operation (popcount, leading/trailing zeros, byte swap, bit reverse)
    IBit(BitOp, VReg, VReg),
    ICmp(Cond, bool, VReg, VReg, VReg), // signed?, d, a, b
    ICmpI(Cond, bool, VReg, VReg, i64),
    FBin(FOp, bool, VReg, VReg, VReg),  // f64?
    FUnary(FUn, bool, VReg, VReg),
    FCmp(Cond, bool, VReg, VReg, VReg),
    Conv(Conv, VReg, VReg),
    Load(Mem, VReg, VReg, i32),  // d, base, offset
    Store(Mem, VReg, i32, VReg), // base, offset, src
    /// backend-only (made by `opt::addr_modes` just before codegen):
    /// d = [base + (idx << shift) + offset]
    LoadX(Mem, VReg, VReg, VReg, u8, i32),
    /// [base + (idx << shift) + offset] = src
    StoreX(Mem, VReg, VReg, u8, i32, VReg),
    SlotAddr(VReg, u32),
    /// a symbol's value (an address of data / a host object, or an embedder index such as a
    /// panic site) plus its offset: code holds it only through a relocation (`Reloc::Sym`)
    Addr(VReg, Sym),
    FnAddr(VReg, u32),
    Call(Callee, Vec<VReg>, Vec<VReg>),
    /// memcpy of a constant size
    Copy(VReg, VReg, u32),
    /// d = c != 0 ? a : b (c an integer vreg; a, b, d of one class)
    Sel(VReg, VReg, VReg, VReg), // d, c, a, b
    /// fused multiply-add, one rounding: d = (neg_prod ? -a : a) * b + (neg_add ? -c : c), the
    /// negations those of the operands (a NaN's sign flips: `(-a).mul_add(b, c)`). A NaN result
    /// is [`fma_nan_f32`] / [`fma_nan_f64`] of the negated operands on every tier and register
    /// assignment (each back end keeps one operand placement).
    /// The only fusion RIR has (the optimiser never fuses).
    FMulAdd(bool, bool, bool, VReg, VReg, VReg, VReg), // neg_prod, neg_add, f64?, d, a, b, c
    /// atomic load (Mem::Int only; the value is extended by signedness)
    AtomicLoad(Mem, AtomOrd, VReg, VReg), // d, addr
    AtomicStore(Mem, AtomOrd, VReg, VReg), // addr, src
    /// d = old value; [addr] = old op src
    AtomicRmw(RmwOp, Mem, AtomOrd, VReg, VReg, VReg), // d, addr, src
    /// compare-exchange: d = old value; stores `new` iff old == expected
    AtomicCas(Mem, AtomOrd, AtomOrd, VReg, VReg, VReg, VReg), // ok ord, fail ord, d, addr, expected, new
    /// memory fence; `true` = compiler-only (single-thread) fence
    Fence(AtomOrd, bool),
    /// the thread's floating-point control register: `FpEnv(false, d)` reads it into d,
    /// `FpEnv(true, x)` writes x to it (x64: MXCSR, 32 bits; arm64: FPCR). Flush-to-zero,
    /// denormals-are-zero, rounding: an effect, ordered like a call
    FpEnv(bool, VReg),
    /// x86 CPUID: four zero-extended u32 results, leaf and subleaf.
    Cpuid([VReg; 4], VReg, VReg),
    /// x86 XGETBV: u64 result and u32 XCR index. Caller checks OSXSAVE.
    Xgetbv(VReg, VReg),
    /// d = address of this thread's copy of the thread-local block at the offset a symbol holds
    TlsAddr(VReg, Sym),
    /// d = the thread pointer + the symbol's value: a variable in the executable's static TLS
    /// (x86-64 Linux: fs:0 + offset, rustc's local-exec model). Pure within a thread, like TlsAddr
    /// never moved (a coroutine may resume on another thread).
    TlsStatic(VReg, Sym),
    // ---- 128-bit vectors (Cls::V128; four 32-bit lanes)
    /// d = bitwise m ? a : b (m: lane masks)
    VSel(VReg, VReg, VReg, VReg), // d, m, a, b
    /// f32 lanes, one rounding: (neg_prod ? -a : a) * b + (neg_add ? -c : c) (operand negations
    /// and NaN lanes as `FMulAdd`)
    VFma(bool, bool, VReg, VReg, VReg, VReg), // neg_prod, neg_add, d, a, b, c
    /// every lane = the low 32 bits of s (an I or F32 vreg)
    VSplat(VReg, VReg),
    /// every lane = a 32-bit constant (a constant-pool load: cheap to rematerialise)
    VConst(VReg, u32),
    /// d = lane `lane` of v: an I vreg gets it sign-extended (i32), an F32 vreg as f32
    VExtract(VReg, VReg, u8),
    /// d = v with lane `lane` replaced by the low 32 bits of s (I or F32)
    VInsert(VReg, VReg, u8, VReg), // d, v, lane, s
    /// 16 bytes at [base + off] (no alignment needed)
    VLoad(VReg, VReg, i32),
    VStore(VReg, i32, VReg), // base, off, src
    /// 16 bytes at [base + (idx << sh) + off] (made by `passes::addr_modes`; sh <= 3, arm64 <= 4)
    VLoadX(VReg, VReg, VReg, u8, i32), // d, base, idx, sh, off
    VStoreX(VReg, VReg, u8, i32, VReg), // base, idx, sh, off, src
    /// d (I) = 1 if any lane of v is nonzero, else 0
    VAny(VReg, VReg),
    /// lane l = the 32-bit word at [base + 4 * idx.lane(l)] (idx lanes as u32, in range)
    VGather(VReg, VReg, VReg), // d, base, idx
    /// d byte k = the byte at [base + bidx.byte(k)] of a small table: n words (n <= 16) at base,
    /// every byte index below 4n (a kernel's constant table read at varying word indices i: bytes
    /// 4i .. 4i + 3; NEON: one ld1 of the table and one TBL)
    VTbl(VReg, VReg, VReg, u8), // d, base, bidx, n
    /// d byte i = (a ++ b) byte idx[i] for idx[i] < 32, else 0 (a constant two-source shuffle;
    /// lane shuffles are their byte indices)
    VShuffle(VReg, VReg, VReg, [u8; 16]), // d, a, b, idx
    /// lane-typed vector op (see `VX`): d, a, b (b repeats a for unary ops)
    VX(VX, VReg, VReg, VReg),
    /// Pure ternary vector op; a is the destructive hardware input, c is explicit in RIR.
    VX3(VX, VReg, VReg, VReg, VReg),
}

#[derive(Clone, PartialEq, Hash, Debug)]
pub enum Term {
    Jump(u32),
    Branch(VReg, u32, u32),
    Ret(Vec<VReg>),
    Unreachable,
    /// (normal, unwind): the block's last instruction is a call (Callee::Fn / Indirect / Host)
    /// that continues at `normal` when it returns and at `unwind` when a panic unwinds out of
    /// it (rustc's `Call { target, unwind: Cleanup(bb) }`). The unwind block (a cleanup pad)
    /// runs drops and ends in a call that resumes unwinding; the call's results are defined on
    /// the normal edge only. The call stays an instruction, so instruction-level passes see it
    /// as any call; passes must keep it last in its block.
    Invoke(u32, u32),
    /// end of a cleanup pad: continue unwinding into the caller (rustc's `UnwindResume`)
    Resume,
    /// Jump table: `targets[x - min]` when `x - min` (wrapping) is below `targets.len()` as an
    /// unsigned number, else `default`. Made by `switch::lower_switches` from a dense integer
    /// match, just before register allocation; the optimiser passes never see it.
    Switch(VReg, std::sync::Arc<SwitchTable>),
}

/// The table of a `Term::Switch`.
#[derive(Clone, Debug, PartialEq, Hash)]
pub struct SwitchTable {
    pub min: i64,
    pub targets: Vec<u32>,
    pub default: u32,
    /// the distinct targets (table entries and default), default last
    pub succ: Vec<u32>,
}

impl SwitchTable {
    pub fn new(min: i64, targets: Vec<u32>, default: u32) -> SwitchTable {
        let mut t = SwitchTable { min, targets, default, succ: Vec::new() };
        t.refresh();
        t
    }
    /// Recomputes `succ` after the targets changed.
    pub fn refresh(&mut self) {
        let mut s: Vec<u32> = Vec::new();
        for &c in &self.targets {
            if c != self.default && !s.contains(&c) {
                s.push(c);
            }
        }
        s.push(self.default);
        self.succ = s;
    }
}

/// A terminator's distinct successor blocks (in order: taken first)
pub struct Succs {
    a: [u32; 2],
    n: u8,
    i: u8,
    sw: Option<std::sync::Arc<SwitchTable>>,
}

impl Iterator for Succs {
    type Item = u32;
    #[inline]
    fn next(&mut self) -> Option<u32> {
        if let Some(t) = &self.sw {
            let r = t.succ.get(self.i as usize).copied();
            self.i += 1;
            return r;
        }
        if self.i < self.n {
            self.i += 1;
            Some(self.a[self.i as usize - 1])
        } else {
            None
        }
    }
}

impl Inst {
    /// The registers this instruction writes (its results), conservatively: where a register
    /// is both read and written (a vector lane insert, the FP control register, Xgetbv) it
    /// counts as written.
    pub fn defs(&self, mut f: impl FnMut(VReg)) {
        match self {
            Inst::Iconst(d, _) | Inst::Fconst(d, _, _) | Inst::SlotAddr(d, _) | Inst::Addr(d, _) | Inst::FnAddr(d, _) | Inst::TlsAddr(d, _) | Inst::TlsStatic(d, _) | Inst::VConst(d, _) => f(*d),
            Inst::Mov(d, _) | Inst::INeg(_, d, _) | Inst::INot(_, d, _) | Inst::IBit(_, d, _) | Inst::FUnary(_, _, d, _) | Inst::Conv(_, d, _) | Inst::IBinI(_, _, d, _, _) | Inst::ICmpI(_, _, d, _, _) | Inst::Load(_, d, _, _) | Inst::VSplat(d, _) | Inst::VExtract(d, _, _) | Inst::VLoad(d, _, _) | Inst::VAny(d, _) | Inst::AtomicLoad(_, _, d, _) => f(*d),
            Inst::IBin(_, _, d, _, _) | Inst::ICmp(_, _, d, _, _) | Inst::FBin(_, _, d, _, _) | Inst::FCmp(_, _, d, _, _) | Inst::VGather(d, _, _) | Inst::VTbl(d, _, _, _) | Inst::VShuffle(d, _, _, _) | Inst::VX(_, d, _, _) | Inst::AtomicRmw(_, _, _, d, _, _) | Inst::LoadX(_, d, _, _, _, _) | Inst::VLoadX(d, _, _, _, _) => f(*d),
            Inst::Sel(d, _, _, _) | Inst::VX3(_, d, _, _, _) | Inst::VSel(d, _, _, _) | Inst::VFma(_, _, d, _, _, _) | Inst::FMulAdd(_, _, _, d, _, _, _) | Inst::AtomicCas(_, _, _, d, _, _, _) => f(*d),
            Inst::VInsert(d, _, _, _) => f(*d),
            Inst::Call(_, _, rets) => rets.iter().for_each(|r| f(*r)),
            Inst::FpEnv(_, v) => f(*v),
            Inst::Cpuid(ds, a, b) => { ds.iter().for_each(|d| f(*d)); f(*a); f(*b); }
            Inst::Xgetbv(d, a) => { f(*d); f(*a); }
            Inst::Store(..) | Inst::Copy(..) | Inst::VStore(..) | Inst::VStoreX(..) | Inst::AtomicStore(..) | Inst::StoreX(..) | Inst::Fence(..) => {}
        }
    }

    /// Every vreg operand and result, in place (renaming without rebuilding the instruction).
    pub fn vregs_mut(&mut self, mut f: impl FnMut(&mut VReg)) {
        match self {
            Inst::Iconst(d, _) | Inst::Fconst(d, _, _) | Inst::SlotAddr(d, _) | Inst::Addr(d, _) | Inst::FnAddr(d, _) | Inst::TlsAddr(d, _) | Inst::TlsStatic(d, _) | Inst::VConst(d, _) => f(d),
            Inst::Mov(d, a) | Inst::INeg(_, d, a) | Inst::INot(_, d, a) | Inst::IBit(_, d, a) | Inst::FUnary(_, _, d, a) | Inst::Conv(_, d, a) | Inst::IBinI(_, _, d, a, _) | Inst::ICmpI(_, _, d, a, _) | Inst::Load(_, d, a, _) | Inst::VSplat(d, a) | Inst::VExtract(d, a, _) | Inst::VLoad(d, a, _) | Inst::VAny(d, a) | Inst::AtomicLoad(_, _, d, a) => {
                f(d);
                f(a);
            }
            Inst::Store(_, a, _, b) | Inst::Copy(a, b, _) | Inst::VStore(a, _, b) | Inst::AtomicStore(_, _, a, b) => {
                f(a);
                f(b);
            }
            Inst::IBin(_, _, d, a, b) | Inst::ICmp(_, _, d, a, b) | Inst::FBin(_, _, d, a, b) | Inst::FCmp(_, _, d, a, b) | Inst::VGather(d, a, b) | Inst::VTbl(d, a, b, _) | Inst::VShuffle(d, a, b, _) | Inst::VX(_, d, a, b) | Inst::AtomicRmw(_, _, _, d, a, b) | Inst::LoadX(_, d, a, b, _, _) | Inst::VLoadX(d, a, b, _, _) => {
                f(d);
                f(a);
                f(b);
            }
            Inst::StoreX(_, a, b, _, _, c) | Inst::VStoreX(a, b, _, _, c) | Inst::VInsert(a, b, _, c) => {
                f(a);
                f(b);
                f(c);
            }
            Inst::Sel(d, a, b, c) | Inst::VX3(_, d, a, b, c) | Inst::VSel(d, a, b, c) | Inst::VFma(_, _, d, a, b, c) | Inst::FMulAdd(_, _, _, d, a, b, c) | Inst::AtomicCas(_, _, _, d, a, b, c) => {
                f(d);
                f(a);
                f(b);
                f(c);
            }
            Inst::Call(c, args, rets) => {
                if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
                    f(v);
                }
                args.iter_mut().for_each(&mut f);
                rets.iter_mut().for_each(&mut f);
            }
            Inst::Fence(..) => {}
            Inst::FpEnv(_, v) => f(v),
            Inst::Cpuid(ds, a, b) => {
                ds.iter_mut().for_each(&mut f);
                f(a);
                f(b);
            }
            Inst::Xgetbv(d, a) => { f(d); f(a); }
        }
    }
}

impl Term {
    /// Every CFG successor, each once. Passes that enumerate edges go through this (and
    /// `map_succs`), so a new terminator kind is one edit here.
    #[inline]
    pub fn succs(&self) -> Succs {
        match *self {
            Term::Jump(t) => Succs { a: [t, 0], n: 1, i: 0, sw: None },
            Term::Branch(_, t, e) | Term::Invoke(t, e) => Succs { a: [t, e], n: if t == e { 1 } else { 2 }, i: 0, sw: None },
            Term::Ret(_) | Term::Unreachable | Term::Resume => Succs { a: [0, 0], n: 0, i: 0, sw: None },
            Term::Switch(_, ref t) => Succs { a: [0, 0], n: 0, i: 0, sw: Some(t.clone()) },
        }
    }
    /// Rewrites every successor block index.
    #[inline]
    pub fn map_succs(&mut self, mut f: impl FnMut(u32) -> u32) {
        match self {
            Term::Jump(t) => *t = f(*t),
            Term::Branch(_, t, e) | Term::Invoke(t, e) => {
                *t = f(*t);
                *e = f(*e);
            }
            Term::Ret(_) | Term::Unreachable | Term::Resume => {}
            Term::Switch(_, t) => {
                let t = std::sync::Arc::make_mut(t);
                for c in &mut t.targets {
                    *c = f(*c);
                }
                t.default = f(t.default);
                t.refresh();
            }
        }
    }
}

#[derive(Clone)]
pub struct Block {
    pub insts: Vec<Inst>,
    /// source position per inst: (file << 32) | byte offset
    pub pos: Vec<u64>,
    pub term: Term,
    pub term_pos: u64,
    /// how often the block ran in the slot's counted build (PGO: the counted tier's exact
    /// count of the O3 input block this one came from; copies keep it), negative = unknown.
    /// A frequency for layout and register allocation only: never a correctness fact.
    pub count: f32,
}

#[derive(Clone)]
pub struct Slot {
    pub size: u32,
    pub align: u32,
    /// made by lowering as one call's by-value argument copy (Relax ABI: an aggregate argument
    /// goes by pointer to a private copy the callee owns): the call keeps no pointer to it past
    /// its return. Only a lowering fact -- passes may give the slot other uses, so its readers
    /// check that the slot is still that (frame::slot_sets)
    pub arg: bool,
    /// made by lowering with its one SlotAddr where its value begins (a temporary, a `let`, a
    /// call's result buffer or argument copy): its earlier contents are dead whenever that
    /// SlotAddr runs (rustc: StorageLive). True of lowering's output only -- passes move and
    /// duplicate SlotAddrs -- so only a baseline body's slots are read as such (opt::slot_colors)
    pub born: bool,
    /// made by lowering as a call's result buffer (a result returned in memory), of the result
    /// type's size: that call writes all of it (opt::slot_colors checks it is still passed to
    /// one call only)
    pub ret: bool,
}

#[derive(Clone)]
pub struct Func {
    pub name: String,
    pub blocks: Vec<Block>,
    pub vregs: Vec<Cls>,
    pub slots: Vec<Slot>,
    /// parameters in ABI order
    pub params: Vec<VReg>,
    /// classes of returned values in ABI order
    pub rets: Vec<Cls>,
    pub file: u32,
    /// parameters holding pointers no other pointer of this function aliases
    /// (`&mut T` params, by-pointer aggregate copies); used by load/store forwarding
    pub noalias: Vec<VReg>,
    /// SPEC-V: the `noalias` parameters of a body built without a clean borrow-check verdict
    /// (`noalias` is empty then): they come back when its verdict is clean and the body is promoted
    pub withheld_noalias: Vec<VReg>,
    /// SPEC-V: the body was lowered as unverified code and sends calls to unverified-mode twins:
    /// once its verdict is clean it is lowered again (verified code calls the slots themselves)
    pub plain_calls: bool,
    /// parameters pointing to at least this many dereferenceable bytes for the whole call
    /// (rustc `dereferenceable(N)` on `&T` / `&mut T` / by-pointer aggregates): loads from
    /// them may be executed speculatively (LICM hoists them out of loops without stores)
    pub deref: Vec<(VReg, u32)>,
    /// Some for an `extern "C" fn` body: params/returns follow the C ABI (see CSig)
    pub cabi: Option<Box<CSig>>,
    /// the parameter holding the caller's result buffer (a result returned in memory): noalias
    /// and nocapture by construction -- the source language cannot name it, only the final
    /// copy or the stores of the result write it (rustc marks it sret noalias nocapture)
    pub sret: Option<VReg>,
    /// callees this function calls with a result buffer as the first argument (their result is
    /// returned in memory): the callee only writes it (noalias, nocapture -- see `sret`), so a
    /// slot passed there is written by the call, not read (forward_slot_dests_x builds such a
    /// result straight where it is copied to)
    pub sret_callees: Vec<u32>,
    /// (callee, argument index, may return it) of calls whose callee keeps no copy of that
    /// pointer argument (codegen opt::param_escape over its body): a frame address passed there
    /// does not escape for sibling calls (tail.rs), the call's result is one when it may be
    /// returned. Filled by the front end for bodies with tail-shaped calls.
    pub nocapture: Vec<(Callee, u32, bool)>,
    /// Nounwind query answers consulted by this body's optimization, including false.
    /// They travel with the immutable O1 body, not with a mutable function slot.
    pub nounwind_inputs: Vec<(u32, bool)>,
    /// This O1 body needs only mandatory legalization before direct codegen.
    /// Retained with the RIR across the startup boundary; O3 clears it.
    pub baseline: bool,
    /// O1 as it runs without O3 (full opt pipeline, Relax core inlined, thorough
    /// register allocation): code marked `// relax: hot`, whose first code is its only code
    /// until O3 replaces it
    pub full: bool,
    /// by-pointer aggregate params (each also in `noalias`): the private copy of a by-value
    /// argument that the callee owns (Relax ABI). Nobody reads it after the call returns or unwinds
    /// out of it (the caller moved it; a cleanup that drops it runs in this function, behind an
    /// Invoke), so a value promoted to a register needs no store before a plain call
    pub owned: Vec<VReg>,
    /// values loaded from a field whose type cannot be null (references, Box, NonNull: the
    /// layout's niche excludes 0) -- the Load that defines each is non-null; other definitions
    /// of the same vreg prove nothing (fold_nonnull checks every definition)
    pub nonnull: Vec<VReg>,
    /// values loaded from a scalar whose type restricts its valid values (`char`, `bool`, enum
    /// tags: the layout's niche, rustc's `!range` load metadata): (vreg, load bytes, lo, hi) --
    /// the zero-extending Load of that width defining the vreg yields a value in lo..=hi; other
    /// definitions of the same vreg prove nothing (value ranges check the defining instruction)
    pub ranged: Vec<(VReg, u8, u64, u64)>,
    /// the trait-object types this body calls methods through (virtual calls, by type id): code
    /// that must run without lazy compiles (real-time threads) compiles their vtables' methods
    pub dyn_calls: Vec<u32>,
    /// a `#[unsafe(naked)]` fn: its one block is its `naked_asm!` (a noreturn Callee::Asm) and
    /// the backend emits no prologue, no epilogue, nothing but the block
    pub naked: bool,
    /// The inline stack (`pos_ctx`): context k (1-based) is entry k-1, (the call's position -- its
    /// own context is the parent's --, a position inside the inlined fn naming it). Machine code
    /// carries it in its pc map (`inline_pc_entries`).
    pub inl: Vec<(u64, u64)>,
    /// Where the source's named locals live (makepad inspect `locals`): side metadata only --
    /// no pass reads it to decide anything, passes that rename vregs or slots map it, and the
    /// back end turns it into per-call-site locations in the pc map (`vars_pc_entries`).
    pub dbg: Vec<DbgVar>,
}

/// A named local of the source and its storage (`Func::dbg`).
#[derive(Clone, Debug, PartialEq, Hash)]
pub struct DbgVar {
    /// its fn's inline context (`pos_ctx`): 0 the function itself, k an inlined copy
    pub ctx: u16,
    /// the local's index in its fn's checked body (`Body::locals`)
    pub local: u32,
    pub loc: DbgLoc,
    /// (leaf, value) of `Regs` leaves the optimiser found constant (their vreg folded away)
    pub k: Vec<(u32, i64)>,
}

/// Storage of a `DbgVar`.
#[derive(Clone, Debug, PartialEq, Hash)]
pub enum DbgLoc {
    /// one vreg per layout leaf (in leaf order); `DBG_NONE` = that leaf is no longer held
    Regs(Vec<VReg>),
    /// the bytes of stack slot `slot` from offset `off`
    Slot(u32, i32),
    /// the bytes at the address a vreg holds, plus `off` (a by-pointer argument copy)
    Mem(VReg, i32),
}

/// A `DbgLoc::Regs` leaf whose value no vreg holds any more.
pub const DBG_NONE: VReg = VReg(u32::MAX);

/// A function slot an `Inst` names (`Callee::Fn` / `Callee::CFn` / `FnAddr`), in place.
pub fn inst_fn_slot_mut(i: &mut Inst) -> Option<&mut u32> {
    match i {
        Inst::Call(Callee::Fn(c) | Callee::CFn(c, _), _, _) | Inst::FnAddr(_, c) => Some(c),
        _ => None,
    }
}

impl Func {
    /// Every function-slot id the body names, in place: calls and fn addresses, `sret_callees`,
    /// the `nocapture` facts' callees and the `nounwind_inputs`. Symbols (`Sym`) are not slots,
    /// even those whose value is a slot's thunk. Relax's back-end memo renames slots through this
    /// (a body equal to another one up to a renaming of its slots compiles to the same code with
    /// the same renaming of its relocations).
    pub fn fn_slots_mut(&mut self, f: &mut dyn FnMut(&mut u32)) {
        for b in &mut self.blocks {
            for i in &mut b.insts {
                if let Some(c) = inst_fn_slot_mut(i) {
                    f(c);
                }
            }
        }
        for c in &mut self.sret_callees {
            f(c);
        }
        for (c, _, _) in &mut self.nocapture {
            if let Callee::Fn(c) | Callee::CFn(c, _) = c {
                f(c);
            }
        }
        for (c, _) in &mut self.nounwind_inputs {
            f(c);
        }
    }

    /// Structural equality of everything but `name`, with the function slots each side names
    /// compared through its own map (`ma` for self, `mb` for `o`; block counts by bits). With
    /// injective maps: the bodies are equal up to a renaming of their slots.
    pub fn same_body_mapped(&self, ma: &mut dyn FnMut(u32) -> u32, o: &Func, mb: &mut dyn FnMut(u32) -> u32) -> bool {
        let Func { name: _, blocks, vregs, slots, params, rets, file, noalias, withheld_noalias, plain_calls, deref, cabi, sret, sret_callees, nocapture, nounwind_inputs, baseline, full, owned, nonnull, ranged, dyn_calls, naked, inl, dbg } = self;
        let callee_eq = |a: &Callee, ma: &mut dyn FnMut(u32) -> u32, b: &Callee, mb: &mut dyn FnMut(u32) -> u32| match (a, b) {
            (Callee::Fn(x), Callee::Fn(y)) => ma(*x) == mb(*y),
            (Callee::CFn(x, xs), Callee::CFn(y, ys)) => ma(*x) == mb(*y) && xs == ys,
            (Callee::Fn(_) | Callee::CFn(..), _) | (_, Callee::Fn(_) | Callee::CFn(..)) => false,
            _ => a == b,
        };
        let inst_eq = |a: &Inst, ma: &mut dyn FnMut(u32) -> u32, b: &Inst, mb: &mut dyn FnMut(u32) -> u32| match (a, b) {
            (Inst::Call(x, xa, xr), Inst::Call(y, ya, yr)) => xa == ya && xr == yr && callee_eq(x, ma, y, mb),
            (Inst::FnAddr(v, x), Inst::FnAddr(w, y)) => v == w && ma(*x) == mb(*y),
            (Inst::FnAddr(..), _) | (_, Inst::FnAddr(..)) => false,
            _ => a == b,
        };
        blocks.len() == o.blocks.len()
            && blocks.iter().zip(&o.blocks).all(|(a, b)| {
                let Block { insts, pos, term, term_pos, count } = a;
                insts.len() == b.insts.len()
                    && insts.iter().zip(&b.insts).all(|(x, y)| inst_eq(x, ma, y, mb))
                    && *pos == b.pos && *term == b.term && *term_pos == b.term_pos && count.to_bits() == b.count.to_bits()
            })
            && *vregs == o.vregs
            && slots.len() == o.slots.len()
            && slots.iter().zip(&o.slots).all(|(a, b)| {
                let Slot { size, align, arg, born, ret } = a;
                (*size, *align, *arg, *born, *ret) == (b.size, b.align, b.arg, b.born, b.ret)
            })
            && *params == o.params
            && *rets == o.rets
            && *file == o.file
            && *noalias == o.noalias
            && *withheld_noalias == o.withheld_noalias
            && *plain_calls == o.plain_calls
            && *deref == o.deref
            && *cabi == o.cabi
            && *sret == o.sret
            && sret_callees.len() == o.sret_callees.len()
            && sret_callees.iter().zip(&o.sret_callees).all(|(x, y)| ma(*x) == mb(*y))
            && nocapture.len() == o.nocapture.len()
            && nocapture.iter().zip(&o.nocapture).all(|(x, y)| x.1 == y.1 && x.2 == y.2 && callee_eq(&x.0, ma, &y.0, mb))
            && nounwind_inputs.len() == o.nounwind_inputs.len()
            && nounwind_inputs.iter().zip(&o.nounwind_inputs).all(|(x, y)| x.1 == y.1 && ma(x.0) == mb(y.0))
            && *baseline == o.baseline
            && *full == o.full
            && *owned == o.owned
            && *nonnull == o.nonnull
            && *ranged == o.ranged
            && *dyn_calls == o.dyn_calls
            && *naked == o.naked
            && *inl == o.inl
            && *dbg == o.dbg
    }

    /// A hash of what `same_body_mapped` compares, the function slots replaced by `m(slot)`.
    pub fn hash_body_mapped<H: std::hash::Hasher>(&self, h: &mut H, m: &mut dyn FnMut(u32) -> u32) {
        use std::hash::Hash;
        let Func { name: _, blocks, vregs, slots, params, rets, file, noalias, withheld_noalias, plain_calls, deref, cabi, sret, sret_callees, nocapture, nounwind_inputs, baseline, full, owned, nonnull, ranged, dyn_calls, naked, inl, dbg } = self;
        fn callee<H: std::hash::Hasher>(c: &Callee, h: &mut H, m: &mut dyn FnMut(u32) -> u32) {
            match c {
                Callee::Fn(x) => {
                    0xf1u8.hash(h);
                    m(*x).hash(h);
                }
                Callee::CFn(x, s) => {
                    0xf2u8.hash(h);
                    m(*x).hash(h);
                    s.hash(h);
                }
                _ => c.hash(h),
            }
        }
        blocks.len().hash(h);
        for Block { insts, pos, term, term_pos, count } in blocks {
            insts.len().hash(h);
            for i in insts {
                match i {
                    Inst::Call(c, a, r) => {
                        0xf0u8.hash(h);
                        callee(c, h, m);
                        a.hash(h);
                        r.hash(h);
                    }
                    Inst::FnAddr(v, x) => {
                        0xf3u8.hash(h);
                        v.hash(h);
                        m(*x).hash(h);
                    }
                    _ => i.hash(h),
                }
            }
            pos.hash(h);
            term.hash(h);
            term_pos.hash(h);
            count.to_bits().hash(h);
        }
        vregs.hash(h);
        slots.len().hash(h);
        for Slot { size, align, arg, born, ret } in slots {
            (size, align, arg, born, ret).hash(h);
        }
        (params, rets, file, noalias, withheld_noalias, plain_calls, deref, cabi, sret).hash(h);
        sret_callees.len().hash(h);
        for x in sret_callees {
            m(*x).hash(h);
        }
        nocapture.len().hash(h);
        for (c, i, r) in nocapture {
            callee(c, h, m);
            (i, r).hash(h);
        }
        nounwind_inputs.len().hash(h);
        for (x, v) in nounwind_inputs {
            m(*x).hash(h);
            v.hash(h);
        }
        (baseline, full, owned, nonnull, ranged, dyn_calls, naked, inl, dbg).hash(h);
    }

    /// Maps the debug table through a vreg renaming (`map(v)` None: v is gone).
    pub fn dbg_map_vregs(&mut self, map: &dyn Fn(VReg) -> Option<VReg>) {
        for d in self.dbg.iter_mut() {
            match &mut d.loc {
                DbgLoc::Regs(r) => {
                    for v in r.iter_mut() {
                        if *v != DBG_NONE {
                            *v = map(*v).unwrap_or(DBG_NONE);
                        }
                    }
                }
                DbgLoc::Mem(v, _) => *v = map(*v).unwrap_or(DBG_NONE),
                DbgLoc::Slot(..) => {}
            }
        }
    }
    /// Live-range splits (register allocation): each (piece, original) pair is a vreg made to
    /// hold the original's value over part of the function. Every debug entry naming the
    /// original gets a twin naming the piece; where a local is looked up, the leaf's vreg live
    /// there (original or piece) holds it.
    pub fn dbg_add_pieces(&mut self, pairs: &[(VReg, VReg)]) {
        if self.dbg.is_empty() || pairs.is_empty() {
            return;
        }
        let mut of: std::collections::HashMap<u32, Vec<VReg>> = std::collections::HashMap::new();
        for &(p, o) in pairs {
            of.entry(o.0).or_default().push(p);
        }
        let mut more = Vec::new();
        for d in &self.dbg {
            if let DbgLoc::Regs(r) = &d.loc {
                for (k, v) in r.iter().enumerate() {
                    for p in of.get(&v.0).map(|x| x.as_slice()).unwrap_or(&[]) {
                        let mut leaves = vec![DBG_NONE; r.len()];
                        leaves[k] = *p;
                        more.push(DbgVar { ctx: d.ctx, local: d.local, loc: DbgLoc::Regs(leaves), k: Vec::new() });
                    }
                }
            }
        }
        self.dbg.extend(more);
    }
    /// A pass proved, for vregs the debug table may name, `d == s` everywhere (`copies`: both
    /// single-definition, d a copy of s) or `d == k` (`consts`: single-definition constants)
    /// before d goes away: its leaves name s, or hold the constant.
    pub fn dbg_salvage(&mut self, copies: &[(VReg, VReg)], consts: &[(VReg, i64)]) {
        if self.dbg.is_empty() || copies.is_empty() && consts.is_empty() {
            return;
        }
        let copies: std::collections::HashMap<u32, VReg> = copies.iter().map(|c| (c.0 .0, c.1)).collect();
        let consts: std::collections::HashMap<u32, i64> = consts.iter().map(|c| (c.0 .0, c.1)).collect();
        for d in self.dbg.iter_mut() {
            match &mut d.loc {
                DbgLoc::Regs(r) => {
                    for (leaf, v) in r.iter_mut().enumerate() {
                        if *v == DBG_NONE {
                            continue;
                        }
                        // (a chain of removed copies: to its end)
                        for _ in 0..64 {
                            match copies.get(&v.0) {
                                Some(&s) if s != *v => *v = s,
                                _ => break,
                            }
                        }
                        if let Some(&k) = consts.get(&v.0) {
                            *v = DBG_NONE;
                            d.k.push((leaf as u32, k));
                        }
                    }
                }
                DbgLoc::Mem(v, _) => {
                    if let Some(&s) = copies.get(&v.0) {
                        *v = s;
                    }
                }
                DbgLoc::Slot(..) => {}
            }
        }
    }

    /// The vregs the debug table names (sorted, deduplicated): passes check removals against it.
    pub fn dbg_vregs(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.dbg.iter().flat_map(|d| match &d.loc {
            DbgLoc::Regs(r) => r.clone(),
            DbgLoc::Mem(a, _) => vec![*a],
            DbgLoc::Slot(..) => Vec::new(),
        }).filter(|v| *v != DBG_NONE).map(|v| v.0).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Maps the debug table through a slot renumbering (`map(s)` None: s is gone).
    pub fn dbg_map_slots(&mut self, map: &dyn Fn(u32) -> Option<u32>) {
        for d in self.dbg.iter_mut() {
            if let DbgLoc::Slot(s, _) = &mut d.loc {
                *s = map(*s).unwrap_or(u32::MAX);
            }
        }
    }
}

/// Source positions are `(file << 32) | byte`, with the inline context in bits 48..63 (0: the
/// function's own body; k: inlined through `Func::inl[k - 1]`). File indices stay below 1 << 16.
pub const POS_CTX_SHIFT: u32 = 48;
/// An inline chain deeper / wider than 16 bits: the context is lost (positions still name lines).
pub const POS_CTX_LOST: u64 = 0xffff;

/// The inline context of a position.
pub fn pos_ctx(p: u64) -> u64 {
    p >> POS_CTX_SHIFT
}

/// A position without its inline context: what names a source place (keys, lookups).
pub fn pos_plain(p: u64) -> u64 {
    p & ((1u64 << POS_CTX_SHIFT) - 1)
}

/// The file index of a position.
pub fn pos_file(p: u64) -> u32 {
    ((p >> 32) & 0xffff) as u32
}

/// `p` with inline context `ctx`.
pub fn pos_with_ctx(p: u64, ctx: u64) -> u64 {
    pos_plain(p) | (ctx.min(POS_CTX_LOST) << POS_CTX_SHIFT)
}

/// The pc-map entries carrying `f`'s inline stack: two per context, at code offset u32::MAX
/// (after every instruction's entry; a lookup never reaches them): the call position, then the
/// position naming the inlined fn. Positions both, so caches translate their files as any other.
pub fn inline_pc_entries(f: &Func) -> impl Iterator<Item = (u32, u64)> + '_ {
    f.inl.iter().flat_map(|&(call, callee)| [(u32::MAX, call), (u32::MAX, callee)])
}

impl Func {
    pub fn new(name: String, file: u32) -> Func {
        Func { name, blocks: Vec::new(), vregs: Vec::new(), slots: Vec::new(), params: Vec::new(), rets: Vec::new(), file, noalias: Vec::new(), withheld_noalias: Vec::new(), plain_calls: false, deref: Vec::new(), cabi: None, sret: None, sret_callees: Vec::new(), nocapture: Vec::new(), nounwind_inputs: Vec::new(), baseline: false, full: false, owned: Vec::new(), nonnull: Vec::new(), ranged: Vec::new(), dyn_calls: Vec::new(), naked: false, inl: Vec::new(), dbg: Vec::new() }
    }
    pub fn vreg(&mut self, c: Cls) -> VReg {
        self.vregs.push(c);
        VReg(self.vregs.len() as u32 - 1)
    }
    pub fn block(&mut self) -> u32 {
        self.blocks.push(Block { insts: Vec::new(), pos: Vec::new(), term: Term::Unreachable, term_pos: 0, count: -1.0 });
        self.blocks.len() as u32 - 1
    }
    pub fn slot(&mut self, size: u32, align: u32) -> u32 {
        self.slots.push(Slot { size: size.max(1), align: align.max(1), arg: false, born: false, ret: false });
        self.slots.len() as u32 - 1
    }

    /// Text dump for debugging and the crash report.
    pub fn dump(&self) -> String {
        let mut s = format!("fn {} params {:?} rets {:?}\n", self.name, self.params, self.rets);
        if self.slots.iter().any(|x| x.size >= 1024) {
            let big: Vec<String> = self.slots.iter().enumerate().filter(|(_, x)| x.size >= 1024).map(|(i, x)| format!("{}:{}", i, x.size)).collect();
            s.push_str(&format!("; slots >= 1 KB: {}\n", big.join(" ")));
        }
        for (i, b) in self.blocks.iter().enumerate() {
            s.push_str(&format!("b{}:\n", i));
            for x in &b.insts {
                s.push_str(&format!("  {:?}\n", x));
            }
            s.push_str(&format!("  {:?}\n", b.term));
        }
        s
    }

    /// C ABI vregs (call arguments or body params) against their CSig.
    fn check_csig(&self, sig: &CSig, vs: &[VReg], what: &str) -> Result<(), String> {
        let skip = sig.ret.is_some() as usize;
        if vs.len() != sig.args.len() + skip {
            return Err(format!("{}: {} ABI values for a C signature of {} (+{} result address)", what, vs.len(), sig.args.len(), skip));
        }
        if skip == 1 && self.vregs[vs[0].0 as usize] != Cls::I {
            return Err(format!("{}: result address is not an integer vreg", what));
        }
        for (k, a) in sig.args.iter().enumerate() {
            let c = self.vregs[vs[skip + k].0 as usize];
            let want = match a {
                CArg::Scalar(Mem::F32) => Cls::F32,
                CArg::Scalar(Mem::F64) => Cls::F64,
                _ => Cls::I,
            };
            if c != want {
                return Err(format!("{}: argument {} is {:?}, its C type needs {:?}", what, k, c, want));
            }
        }
        Ok(())
    }

    /// Structural verifier: every use has a class-correct vreg, every target exists.
    pub fn verify(&self) -> Result<(), String> {
        // a naked fn's body is its asm: it has no parameter vregs
        if let Some(sig) = self.cabi.as_ref().filter(|_| !self.naked) {
            for p in &self.params {
                if p.0 as usize >= self.vregs.len() {
                    return Err(format!("{}: param vreg out of range", self.name));
                }
            }
            self.check_csig(sig, &self.params, &format!("{}: extern \"C\" params", self.name))?;
            if sig.ret.is_some() && !self.rets.is_empty() {
                return Err(format!("{}: aggregate C result and result registers", self.name));
            }
        }
        let nb = self.blocks.len() as u32;
        let nv = self.vregs.len() as u32;
        let chk = |v: VReg| -> Result<(), String> {
            if v.0 >= nv {
                return Err(format!("{}: vreg v{} out of range", self.name, v.0));
            }
            Ok(())
        };
        for (bi, b) in self.blocks.iter().enumerate() {
            for x in &b.insts {
                let mut regs: Vec<VReg> = Vec::new();
                match x {
                    Inst::Iconst(d, _) | Inst::Fconst(d, _, _) | Inst::SlotAddr(d, _) | Inst::Addr(d, _) | Inst::FnAddr(d, _) | Inst::VConst(d, _) => regs.push(*d),
                    Inst::Mov(d, a) | Inst::INeg(_, d, a) | Inst::INot(_, d, a) | Inst::IBit(_, d, a) | Inst::FUnary(_, _, d, a) | Inst::Conv(_, d, a) | Inst::IBinI(_, _, d, a, _) | Inst::ICmpI(_, _, d, a, _) => {
                        regs.push(*d);
                        regs.push(*a);
                    }
                    Inst::IBin(_, _, d, a, b) | Inst::ICmp(_, _, d, a, b) | Inst::FBin(_, _, d, a, b) | Inst::FCmp(_, _, d, a, b) => {
                        regs.push(*d);
                        regs.push(*a);
                        regs.push(*b);
                    }
                    Inst::Load(_, d, b, _) => {
                        regs.push(*d);
                        regs.push(*b);
                    }
                    Inst::Store(_, b, _, s) => {
                        regs.push(*b);
                        regs.push(*s);
                    }
                    Inst::LoadX(_, d, b, x, _, _) | Inst::VLoadX(d, b, x, _, _) => {
                        regs.push(*d);
                        regs.push(*b);
                        regs.push(*x);
                    }
                    Inst::StoreX(_, b, x, _, _, s) | Inst::VStoreX(b, x, _, _, s) => {
                        regs.push(*b);
                        regs.push(*x);
                        regs.push(*s);
                    }
                    Inst::Call(c, a, r) => {
                        if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
                            regs.push(*v);
                        }
                        regs.extend_from_slice(a);
                        regs.extend_from_slice(r);
                    }
                    Inst::Copy(a, b, _) => {
                        regs.push(*a);
                        regs.push(*b);
                    }
                    Inst::Fence(..) => {}
                    Inst::FpEnv(_, v) => regs.push(*v),
                    Inst::Cpuid(ds, a, b) => { regs.extend_from_slice(ds); regs.push(*a); regs.push(*b); }
                    Inst::Xgetbv(d, a) => { regs.push(*d); regs.push(*a); }
                    Inst::TlsAddr(d, _) | Inst::TlsStatic(d, _) => regs.push(*d),
                    Inst::VSplat(d, a) | Inst::VExtract(d, a, _) | Inst::VLoad(d, a, _) | Inst::VStore(d, _, a) | Inst::VAny(d, a) => {
                        regs.push(*d);
                        regs.push(*a);
                    }
                    Inst::VGather(d, a, b) | Inst::VTbl(d, a, b, _) | Inst::VShuffle(d, a, b, _) => {
                        regs.push(*d);
                        regs.push(*a);
                        regs.push(*b);
                    }
                    Inst::VX(_, d, a, b) => {
                        regs.push(*d);
                        regs.push(*a);
                        regs.push(*b);
                    }
                    Inst::VX3(_, d, a, b, c) | Inst::VSel(d, a, b, c) | Inst::VFma(_, _, d, a, b, c) => {
                        regs.push(*d);
                        regs.push(*a);
                        regs.push(*b);
                        regs.push(*c);
                    }
                    Inst::VInsert(d, v, _, s2) => {
                        regs.push(*d);
                        regs.push(*v);
                        regs.push(*s2);
                    }
                    Inst::Sel(d, c, a, b) => {
                        regs.push(*d);
                        regs.push(*c);
                        regs.push(*a);
                        regs.push(*b);
                    }
                    Inst::FMulAdd(_, _, _, d, a, b, c) => {
                        regs.push(*d);
                        regs.push(*a);
                        regs.push(*b);
                        regs.push(*c);
                    }
                    Inst::AtomicLoad(_, _, d, a) | Inst::AtomicStore(_, _, d, a) => {
                        regs.push(*d);
                        regs.push(*a);
                    }
                    Inst::AtomicRmw(_, _, _, d, a, b) => {
                        regs.push(*d);
                        regs.push(*a);
                        regs.push(*b);
                    }
                    Inst::AtomicCas(_, _, _, d, a, b, c) => {
                        regs.push(*d);
                        regs.push(*a);
                        regs.push(*b);
                        regs.push(*c);
                    }
                }
                for r in regs {
                    chk(r)?;
                }
                if let Inst::Call(Callee::CHost(_, sig) | Callee::CIndirect(_, sig) | Callee::CFn(_, sig), a, r) = x {
                    let what = format!("{}: b{} C call", self.name, bi);
                    self.check_csig(sig, a, &what)?;
                    if sig.ret.is_some() && !r.is_empty() {
                        return Err(format!("{}: aggregate result and result vregs", what));
                    }
                }
                match x {
                    Inst::IBin(_, _, d, a, b) | Inst::ICmp(_, _, d, a, b) => {
                        if self.vregs[a.0 as usize] != Cls::I || self.vregs[b.0 as usize] != Cls::I || self.vregs[d.0 as usize] != Cls::I {
                            return Err(format!("{}: b{} int op on non-int vreg: {:?}", self.name, bi, x));
                        }
                    }
                    Inst::FBin(_, f64_, d, a, b) => {
                        let c = if *f64_ { Cls::F64 } else { Cls::F32 };
                        if self.vregs[a.0 as usize] != c || self.vregs[b.0 as usize] != c || self.vregs[d.0 as usize] != c {
                            return Err(format!("{}: b{} float op class mismatch: {:?}", self.name, bi, x));
                        }
                    }
                    Inst::Sel(d, c, a, b2) => {
                        let cd = self.vregs[d.0 as usize];
                        if self.vregs[c.0 as usize] != Cls::I || self.vregs[a.0 as usize] != cd || self.vregs[b2.0 as usize] != cd {
                            return Err(format!("{}: b{} select class mismatch: {:?}", self.name, bi, x));
                        }
                    }
                    _ => {}
                }
            }
            match &b.term {
                Term::Jump(t) => {
                    if *t >= nb {
                        return Err(format!("{}: bad jump target", self.name));
                    }
                }
                Term::Branch(c, t, f) => {
                    chk(*c)?;
                    if *t >= nb || *f >= nb {
                        return Err(format!("{}: bad branch target", self.name));
                    }
                }
                Term::Ret(v) => {
                    if v.len() != self.rets.len() {
                        return Err(format!("{}: b{} returns {} values, expected {}", self.name, bi, v.len(), self.rets.len()));
                    }
                    for (i, r) in v.iter().enumerate() {
                        chk(*r)?;
                        if self.vregs[r.0 as usize] != self.rets[i] {
                            return Err(format!("{}: b{} return class mismatch", self.name, bi));
                        }
                    }
                }
                Term::Unreachable | Term::Resume => {}
                Term::Switch(c, t) => {
                    chk(*c)?;
                    if t.succ.iter().any(|s| *s >= nb) {
                        return Err(format!("{}: bad switch target", self.name));
                    }
                }
                Term::Invoke(t, u) => {
                    if *t >= nb || *u >= nb {
                        return Err(format!("{}: bad invoke target", self.name));
                    }
                    if !matches!(b.insts.last(), Some(Inst::Call(Callee::Fn(_) | Callee::Indirect(_) | Callee::Host(_), _, _))) && !matches!(b.insts.last(), Some(Inst::Call(Callee::CFn(_, s) | Callee::CIndirect(_, s), _, _)) if s.unwinds) {
                        return Err(format!("{}: b{} invoke without a call that may unwind as its last instruction", self.name, bi));
                    }
                }
            }
        }
        Ok(())
    }
}

/// `v` re-extended to integer type `it` (how RIR keeps narrow integers in registers).
/// The NaN result of `FMulAdd` / `VFma` lanes: `fma(a, b, c)` with the operands already negated
/// (`neg_prod` / `neg_add`), on this host (Rust leaves it unspecified; the back ends fix it so
/// that every tier and register assignment gives the same bits, and compute kernels match their
/// interpreter, makepad_script_compute::ir::fma_nan):
/// - x86-64: the first NaN of b, a, c, quieted (vfmadd213 with a in the destination, b in vvvv,
///   c in r/m -- glibc's fma/fmaf, what rustc calls without the fma feature; the vf*231/132 forms
///   keep b first too); no NaN operand (inf * 0, inf - inf): the negative default NaN.
/// - AArch64 (fmadd/fmla: Rn = a, Rm = b, addend c; Arm's FPMulAdd): the first signaling NaN of
///   c, a, b, else the first quiet NaN of c, a, b, quieted -- except that a quiet NaN c with
///   inf * 0 gives the default NaN; none: the positive default NaN.
/// - wasm: as the engine's f64 multiply then add pass NaNs on (`b * a + c`, the x86-64 order on
///   x86-64 engines); wasm itself leaves NaN signs nondeterministic.
/// `None` when the result is not a NaN.
pub fn fma_nan_f32(a: f32, b: f32, c: f32) -> Option<u32> {
    let r = a.mul_add(b, c);
    fma_nan(a.to_bits() as u64, b.to_bits() as u64, c.to_bits() as u64, 23, 8, r.is_nan()).map(|x| x as u32)
}

/// [`fma_nan_f32`] for f64.
pub fn fma_nan_f64(a: f64, b: f64, c: f64) -> Option<u64> {
    let r = a.mul_add(b, c);
    fma_nan(a.to_bits(), b.to_bits(), c.to_bits(), 52, 11, r.is_nan())
}

/// m mantissa and e exponent bits; `is_nan`: the result's value is a NaN
fn fma_nan(a: u64, b: u64, c: u64, m: u32, e: u32, is_nan: bool) -> Option<u64> {
    if !is_nan {
        return None;
    }
    let exp = ((1u64 << e) - 1) << m;
    let abs = |x: u64| x & (exp | ((1 << m) - 1));
    let nan = |x: u64| abs(x) > exp;
    let quiet = 1u64 << (m - 1);
    let default = exp | quiet;
    #[cfg(target_arch = "aarch64")]
    {
        let snan = |x: u64| nan(x) && x & quiet == 0;
        if let Some(&x) = [c, a, b].iter().find(|&&x| snan(x)) {
            return Some(x | quiet);
        }
        let (inf, zero) = (|x: u64| abs(x) == exp, |x: u64| abs(x) == 0);
        if nan(c) && ((inf(a) && zero(b)) || (zero(a) && inf(b))) {
            return Some(default);
        }
        Some([c, a, b].iter().find(|&&x| nan(x)).map_or(default, |&x| x | quiet))
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let sign = 1u64 << (m + e);
        let default = if cfg!(target_arch = "x86_64") { sign | default } else { default };
        Some([b, a, c].iter().find(|&&x| nan(x)).map_or(default, |&x| x | quiet))
    }
}

pub fn extend_const(v: i64, it: IntTy) -> i64 {
    match (it.bits, it.signed) {
        (8, true) => v as i8 as i64,
        (8, false) => v as u8 as i64,
        (16, true) => v as i16 as i64,
        (16, false) => v as u16 as i64,
        (32, true) => v as i32 as i64,
        (32, false) => v as u32 as i64,
        _ => v,
    }
}

/// Trailer every compiled function ends with, for the runtime's frame walk when a panic
/// unwinds (rustc/LLVM keep the same facts in the LSDA and CFI): the callee-saved registers the
/// prologue saved below the frame pointer ([fp - 8k], k = 1.. in this order; 0x100 | n for
/// float register n), the frame pointer minus the stack pointer in the body, and the landing
/// stub of each invoked call by its return address (byte offsets in the code). Layout (u32
/// words, read backwards from the end): (ret, stub).., saved registers as bytes (n | 0x80 for
/// a float register) four per word, sp_off, then UNWIND_MAGIC << 24 | n_saved << 19 | n_pads.
pub const UNWIND_MAGIC: u32 = 0xA5;

pub fn unwind_info_words(saved: &[u32], pads: &[(u32, u32)], sp_off: u32) -> Vec<u32> {
    let mut w = Vec::with_capacity(2 * pads.len() + saved.len() / 4 + 3);
    for (r, s) in pads {
        w.push(*r);
        w.push(*s);
    }
    for chunk in saved.chunks(4) {
        let mut x = 0u32;
        for (k, r) in chunk.iter().enumerate() {
            let b = if *r & 0x100 != 0 { 0x80 | (*r & 0x7f) } else { *r & 0x7f };
            x |= b << (8 * k);
        }
        w.push(x);
    }
    w.push(sp_off);
    w.push(UNWIND_MAGIC << 24 | (saved.len() as u32) << 19 | pads.len() as u32);
    w
}

/// Parsed trailer of a function's code (see `unwind_info_words`).
pub struct UnwindInfo {
    pub saved: Vec<u32>,
    pub pads: Vec<(u32, u32)>,
    pub sp_off: u32,
}

/// Reads the trailer at the end of `code` (`len` bytes at `code`, a multiple of 4).
///
/// # Safety
/// `code..code + len` must be readable.
pub unsafe fn read_unwind_info(code: u64, len: u64) -> Option<UnwindInfo> {
    if len < 8 {
        return None;
    }
    let rd = |off: u64| -> u32 { std::ptr::read_unaligned((code + off) as *const u32) };
    let end = len & !3;
    let head = rd(end - 4);
    if head >> 24 != UNWIND_MAGIC {
        return None;
    }
    let n_saved = ((head >> 19) & 0x1f) as u64;
    let n_pads = (head & 0x7_ffff) as u64;
    let sp_off = rd(end - 8);
    let saved_words = (n_saved + 3) / 4;
    let words = 2 * n_pads + saved_words + 2;
    if words * 4 > end {
        return None;
    }
    let base = end - words * 4;
    let pads = (0..n_pads).map(|k| (rd(base + 8 * k), rd(base + 8 * k + 4))).collect();
    let sbase = base + 8 * n_pads;
    let saved = (0..n_saved)
        .map(|k| {
            let b = (rd(sbase + 4 * (k / 4)) >> (8 * (k % 4))) & 0xff;
            if b & 0x80 != 0 {
                0x100 | (b & 0x7f)
            } else {
                b
            }
        })
        .collect();
    Some(UnwindInfo { saved, pads, sp_off })
}

/// Splits a compiled function into its code and its unwind trailer: (code length without
/// the trailer, the trailer). The runtime keeps the trailer out of the code memory.
pub fn split_unwind_info(code: &[u8]) -> (usize, Option<UnwindInfo>) {
    let n = code.len() & !3;
    if n < 8 {
        return (code.len(), None);
    }
    let info = unsafe { read_unwind_info(code.as_ptr() as u64, n as u64) };
    match info {
        Some(i) => {
            let words = 2 * i.pads.len() + (i.saved.len() + 3) / 4 + 2;
            (n - 4 * words, Some(i))
        }
        None => (code.len(), None),
    }
}

/// MAKEPAD_RUN_UNWIND_STATS: bytes of cleanup pads, landing stubs, unwind trailers; invoke count.
pub static UNWIND_STATS: [std::sync::atomic::AtomicU64; 4] = [const { std::sync::atomic::AtomicU64::new(0) }; 4];

pub fn unwind_stats_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_RUN_UNWIND_STATS").is_ok())
}

/// Per stack slot: whether any SlotAddr names it (the only way to reach a slot).
pub fn used_slots(f: &Func) -> Vec<bool> {
    let mut used = vec![false; f.slots.len()];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::SlotAddr(_, s) = i {
                if let Some(u) = used.get_mut(*s as usize) {
                    *u = true;
                }
            }
        }
    }
    used
}

#[cfg(test)]
mod slot_table_tests {
    use super::SlotTable;

    #[test]
    fn locate_covers_every_id_once() {
        // segment boundaries: the last id of a segment and the first of the next
        let mut next = 0u64;
        for seg in 0..super::SLOT_SEGS {
            assert_eq!(SlotTable::seg_first(seg), next);
            let n = SlotTable::seg_slots(seg) as u64;
            for id in [next, next + 1, next + n - 1] {
                assert_eq!(SlotTable::locate(id as u32), (seg, (id - next) as usize), "id {}", id);
            }
            next += n;
        }
        assert_eq!(next, 1 << 32);
    }

    #[test]
    fn addresses_and_reverse_lookup() {
        let t = SlotTable::new(32, 32, 8, 48);
        let (a, b) = (0x10_0000u64, 0x4000_0000u64);
        t.install(0, a, a + (1 << 18) * 32, a + (1 << 19) * 32, a + (1 << 19) * 32 + (1 << 18) * 8);
        t.install(1, b, b + (1 << 18) * 32, b + (1 << 19) * 32, b + (1 << 19) * 32 + (1 << 18) * 8);
        assert_eq!(t.thunk(5), a + 5 * 32);
        assert_eq!(t.thunk(1 << 18), b);
        assert_eq!(t.table((1 << 18) + 3), b + (1 << 19) * 32 + 24);
        assert_eq!(t.find(2, t.table((1 << 18) + 3)), Some(((1 << 18) + 3, 0)));
        assert_eq!(t.find(0, a + 5 * 32 + 4), Some((5, 4)));
        assert_eq!(t.find(0, 0x2000_0000), None);
    }
}
