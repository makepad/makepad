//! C-callable stubs inside libmakepad.so. Android's system libraries (libaaudio, ...) are built
//! with CFI: an indirect call through a function pointer is checked against the code of the
//! libraries loaded, and a pointer into the JIT's memory crashes it (`__cfi_slowpath`). Relax gives C
//! code an address in this table's code instead for every C-ABI function it hands out
//! (os::proc::trampoline): stub i loads word i of the table and jumps there; Relax writes the
//! JIT thunk's address into the word.

use std::arch::global_asm;

pub const STUBS: usize = 4096;

// 16 bytes a stub: adrp, ldr, br, nop
global_asm!(
    ".text",
    ".balign 16",
    ".globl makepad_stubs",
    ".hidden makepad_stubs",
    "makepad_stubs:",
    ".set stub_index, 0",
    ".rept 4096",
    "adrp x16, makepad_stub_table + stub_index * 8",
    "ldr x16, [x16, :lo12:makepad_stub_table + stub_index * 8]",
    "br x16",
    "nop",
    ".set stub_index, stub_index + 1",
    ".endr",
    ".bss",
    ".balign 8",
    ".globl makepad_stub_table",
    ".hidden makepad_stub_table",
    "makepad_stub_table:",
    ".zero 32768",
    ".text",
);

extern "C" {
    static makepad_stubs: u8;
    static mut makepad_stub_table: [u64; STUBS];
}

/// Names the stubs to Relax.
pub fn register() {
    relax_compiler::os::proc::set_trampolines(
        std::ptr::addr_of!(makepad_stubs) as u64,
        std::ptr::addr_of_mut!(makepad_stub_table) as *mut u64,
        STUBS,
    );
}
