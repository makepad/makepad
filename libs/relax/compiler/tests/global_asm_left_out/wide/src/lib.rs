// A crate whose global_asm! names a zmm register, which needs EVEX: Relax's assembler does not
// encode it, so Relax leaves this crate's blocks out. The probe returns before the AVX-512
// instruction, so it runs on any x86_64 CPU under rustc.
#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
std::arch::global_asm!(
    ".globl gal_wide_probe",
    "gal_wide_probe:",
    "  xor eax, eax",
    "  ret",
    "  vmovdqu64 zmm0, [rdi]",
);

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
extern "C" {
    fn gal_wide_probe() -> u32;
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
pub fn probe() -> u32 {
    unsafe { gal_wide_probe() }
}
