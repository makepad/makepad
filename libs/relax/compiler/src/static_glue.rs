// Precompiled runtime glue for no-codegen mode (`RELAX_NO_CODEGEN`, the platforms without
// writable+executable memory): every piece of glue the runtime otherwise emits into code memory
// at startup (codegen arm64.rs: lazy_entry, enter_leave, panic_tramp, land, thunk, stub) as code
// in this binary, with the same register contracts. Included into jit.rs (`#[path]` module), so
// it names the runtime entries it jumps to directly.
//
// Slot thunks and lazy stubs are pools sized at build time: segment 0 of the slot table
// (SlotTable::seg_slots(0) = 262,144 slots), 8 bytes each: `adr x16, .` (its own address) and a
// branch to the pool's common code, which makes the slot from the address. Thunk k then loads slot
// k's word from the static table (`ldr`, then `br`); stub k passes k in x16 to the lazy entry. Both
// only use x16/x17, the registers AAPCS64 leaves to veneers, as the generated ones do. (16-byte
// entries with the slot as immediates were 8 MB of the iOS binary's text; these are 4 MB.)

use super::{rt_compile_global, rt_hot, rt_panic_bounds, rt_panic_impl, rt_panic_site, rt_resume_unwind};

/// Slots the pools cover (segment 0 of the slot table).
pub(super) const SLOTS: usize = 1 << 18;
/// Bytes per thunk and per stub (jit.rs NC_STRIDE).
const _: () = assert!(super::NC_STRIDE == 8);

/// The function table of segment 0 (one word per slot: its current code).
#[no_mangle]
static mut RELAX_SG_TABLE: [u64; SLOTS] = [0; SLOTS];

std::arch::global_asm!(
    ".text",
    // ---- slot thunks: x16 = the thunk's address; x16 - thunks = 8k = table[k]'s offset; br
    ".p2align 3",
    "_relax_sg_thunk_common:",
    "  adrp x17, _relax_sg_thunks@PAGE",
    "  add x17, x17, _relax_sg_thunks@PAGEOFF",
    "  sub x16, x16, x17",
    "  adrp x17, {table}@PAGE",
    "  add x17, x17, {table}@PAGEOFF",
    "  ldr x16, [x17, x16]",
    "  br x16",
    ".p2align 3",
    ".globl _relax_sg_thunks",
    "_relax_sg_thunks:",
    ".rept 262144",
    "  adr x16, .",
    "  b _relax_sg_thunk_common",
    ".endr",
    // ---- lazy stubs: x16 = k ((address - stubs) / 8); the lazy entry compiles slot k and
    // continues in its code
    ".p2align 3",
    "_relax_sg_stub_common:",
    "  adrp x17, _relax_sg_stubs@PAGE",
    "  add x17, x17, _relax_sg_stubs@PAGEOFF",
    "  sub x16, x16, x17",
    "  lsr x16, x16, #3",
    "  b _relax_sg_lazy_entry",
    ".p2align 3",
    ".globl _relax_sg_stubs",
    "_relax_sg_stubs:",
    ".rept 262144",
    "  adr x16, .",
    "  b _relax_sg_stub_common",
    ".endr",
    // ---- lazy entry (x16 = global slot: the window it is in names the unit, jit.rs
    // nc_window_of): saves the argument registers (q0-q7 whole), x8; rt_compile_global
    // returns the address to continue at
    ".p2align 2",
    ".globl _relax_sg_lazy_entry",
    "_relax_sg_lazy_entry:",
    "  stp x29, x30, [sp, #-16]!",
    "  mov x29, sp",
    "  sub sp, sp, #208",
    "  stp x0, x1, [sp, #0]",
    "  stp x2, x3, [sp, #16]",
    "  stp x4, x5, [sp, #32]",
    "  stp x6, x7, [sp, #48]",
    "  stp q0, q1, [sp, #64]",
    "  stp q2, q3, [sp, #96]",
    "  stp q4, q5, [sp, #128]",
    "  stp q6, q7, [sp, #160]",
    "  str x8, [sp, #192]",
    "  mov x0, x16",
    "  bl {rt_compile}",
    "  mov x16, x0",
    "  ldp x0, x1, [sp, #0]",
    "  ldp x2, x3, [sp, #16]",
    "  ldp x4, x5, [sp, #32]",
    "  ldp x6, x7, [sp, #48]",
    "  ldp q0, q1, [sp, #64]",
    "  ldp q2, q3, [sp, #96]",
    "  ldp q4, q5, [sp, #128]",
    "  ldp q6, q7, [sp, #160]",
    "  ldr x8, [sp, #192]",
    "  mov sp, x29",
    "  ldp x29, x30, [sp], #16",
    "  br x16",
    // ---- hot entry: the same with rt_hot (generated count stubs' path; the interpreter's count
    // stubs call rt_hot themselves: interp::count_entry)
    ".p2align 2",
    ".globl _relax_sg_hot_entry",
    "_relax_sg_hot_entry:",
    "  stp x29, x30, [sp, #-16]!",
    "  mov x29, sp",
    "  sub sp, sp, #208",
    "  stp x0, x1, [sp, #0]",
    "  stp x2, x3, [sp, #16]",
    "  stp x4, x5, [sp, #32]",
    "  stp x6, x7, [sp, #48]",
    "  stp q0, q1, [sp, #64]",
    "  stp q2, q3, [sp, #96]",
    "  stp q4, q5, [sp, #128]",
    "  stp q6, q7, [sp, #160]",
    "  str x8, [sp, #192]",
    "  mov x0, x16",
    "  bl {rt_hot}",
    "  mov x16, x0",
    "  ldp x0, x1, [sp, #0]",
    "  ldp x2, x3, [sp, #16]",
    "  ldp x4, x5, [sp, #32]",
    "  ldp x6, x7, [sp, #48]",
    "  ldp q0, q1, [sp, #64]",
    "  ldp q2, q3, [sp, #96]",
    "  ldp q4, q5, [sp, #128]",
    "  ldp q6, q7, [sp, #160]",
    "  ldr x8, [sp, #192]",
    "  mov sp, x29",
    "  ldp x29, x30, [sp], #16",
    "  br x16",
    // ---- enter(fn = x0, arg = x1, ctx = x2): saves the callee-saved registers, ctx[0] = sp,
    // calls fn(arg); returns 0. leave(ctx = x0, code = x1): back to that enter, which returns code.
    ".p2align 2",
    ".globl _relax_sg_enter",
    "_relax_sg_enter:",
    "  stp x29, x30, [sp, #-16]!",
    "  mov x29, sp",
    "  sub sp, sp, #160",
    "  stp x19, x20, [sp, #0]",
    "  stp x21, x22, [sp, #16]",
    "  stp x23, x24, [sp, #32]",
    "  stp x25, x26, [sp, #48]",
    "  stp x27, x28, [sp, #64]",
    "  stp d8, d9, [sp, #80]",
    "  stp d10, d11, [sp, #96]",
    "  stp d12, d13, [sp, #112]",
    "  stp d14, d15, [sp, #128]",
    "  mov x16, sp",
    "  str x16, [x2]",
    "  mov x16, x0",
    "  mov x0, x1",
    "  blr x16",
    "  mov x0, #0",
    "Lrelax_sg_enter_tail:",
    "  ldp x19, x20, [sp, #0]",
    "  ldp x21, x22, [sp, #16]",
    "  ldp x23, x24, [sp, #32]",
    "  ldp x25, x26, [sp, #48]",
    "  ldp x27, x28, [sp, #64]",
    "  ldp d8, d9, [sp, #80]",
    "  ldp d10, d11, [sp, #96]",
    "  ldp d12, d13, [sp, #112]",
    "  ldp d14, d15, [sp, #128]",
    "  add sp, sp, #160",
    "  ldp x29, x30, [sp], #16",
    "  ret",
    ".globl _relax_sg_leave",
    "_relax_sg_leave:",
    "  ldr x16, [x0]",
    "  mov sp, x16",
    "  mov x0, x1",
    "  b Lrelax_sg_enter_tail",
    // ---- panic trampolines: the caller's register block [fp, lr, x19..x28, d8..d15, x0..x15]
    // on the stack, its address in x6, x7 = 0, then the host entry (never returns)
    ".macro relax_sg_tramp name, target",
    ".p2align 2",
    ".globl \\name",
    "\\name:",
    "  sub sp, sp, #288",
    "  stp x29, x30, [sp, #0]",
    "  stp x19, x20, [sp, #16]",
    "  stp x21, x22, [sp, #32]",
    "  stp x23, x24, [sp, #48]",
    "  stp x25, x26, [sp, #64]",
    "  stp x27, x28, [sp, #80]",
    "  stp d8, d9, [sp, #96]",
    "  stp d10, d11, [sp, #112]",
    "  stp d12, d13, [sp, #128]",
    "  stp d14, d15, [sp, #144]",
    "  stp x0, x1, [sp, #160]",
    "  stp x2, x3, [sp, #176]",
    "  stp x4, x5, [sp, #192]",
    "  stp x6, x7, [sp, #208]",
    "  stp x8, x9, [sp, #224]",
    "  stp x10, x11, [sp, #240]",
    "  stp x12, x13, [sp, #256]",
    "  stp x14, x15, [sp, #272]",
    "  mov x6, sp",
    "  mov x7, #0",
    "  adrp x16, \\target@PAGE",
    "  add x16, x16, \\target@PAGEOFF",
    "  br x16",
    ".endm",
    "relax_sg_tramp _relax_sg_panic_site, {panic_site}",
    "relax_sg_tramp _relax_sg_panic_bounds, {panic_bounds}",
    "relax_sg_tramp _relax_sg_panic_impl, {panic_impl}",
    "relax_sg_tramp _relax_sg_resume, {resume}",
    // ---- land(regs = x0, fp = x1, sp = x2, pc = x3): continues at pc with x19..x28, d8..d15
    // from the register block, x29 = fp, sp = sp
    ".p2align 2",
    ".globl _relax_sg_land",
    "_relax_sg_land:",
    "  ldp x19, x20, [x0, #16]",
    "  ldp x21, x22, [x0, #32]",
    "  ldp x23, x24, [x0, #48]",
    "  ldp x25, x26, [x0, #64]",
    "  ldp x27, x28, [x0, #80]",
    "  ldp d8, d9, [x0, #96]",
    "  ldp d10, d11, [x0, #112]",
    "  ldp d12, d13, [x0, #128]",
    "  ldp d14, d15, [x0, #144]",
    "  mov x29, x1",
    "  mov sp, x2",
    "  br x3",
    table = sym RELAX_SG_TABLE,
    rt_compile = sym rt_compile_global,
    rt_hot = sym rt_hot,
    panic_site = sym rt_panic_site,
    panic_bounds = sym rt_panic_bounds,
    panic_impl = sym rt_panic_impl,
    resume = sym rt_resume_unwind,
);

extern "C" {
    static relax_sg_thunks: u8;
    static relax_sg_stubs: u8;
    static relax_sg_lazy_entry: u8;
    static relax_sg_hot_entry: u8;
    static relax_sg_enter: u8;
    static relax_sg_leave: u8;
    static relax_sg_panic_site: u8;
    static relax_sg_panic_bounds: u8;
    static relax_sg_panic_impl: u8;
    static relax_sg_resume: u8;
    static relax_sg_land: u8;
}

/// The addresses of the precompiled glue.
pub(super) struct Glue {
    pub thunks: u64,
    pub stubs: u64,
    pub table: u64,
    pub lazy_entry: u64,
    pub hot_entry: u64,
    pub enter: u64,
    pub leave: u64,
    pub panic_site: u64,
    pub panic_bounds: u64,
    pub panic_impl: u64,
    pub resume: u64,
    pub land: u64,
}

pub(super) fn glue() -> Glue {
    fn a(s: &u8) -> u64 {
        s as *const u8 as u64
    }
    unsafe {
        Glue {
            thunks: a(&relax_sg_thunks),
            stubs: a(&relax_sg_stubs),
            table: std::ptr::addr_of!(RELAX_SG_TABLE) as u64,
            lazy_entry: a(&relax_sg_lazy_entry),
            hot_entry: a(&relax_sg_hot_entry),
            enter: a(&relax_sg_enter),
            leave: a(&relax_sg_leave),
            panic_site: a(&relax_sg_panic_site),
            panic_bounds: a(&relax_sg_panic_bounds),
            panic_impl: a(&relax_sg_panic_impl),
            resume: a(&relax_sg_resume),
            land: a(&relax_sg_land),
        }
    }
}

/// The interpreter's source of the thread's thread-local block (no `tls_slow` glue here).
pub(super) extern "C" fn tls_block_c() -> u64 {
    super::tls_block()
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    RELAX_SG_TABLE = [0; SLOTS];
}
