// A crate whose global_asm! jumps to gal-wide's label: with gal-wide's blocks left out, Relax
// leaves this crate's out too.
#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
std::arch::global_asm!(
    ".globl gal_user_call",
    "gal_user_call:",
    "  jmp gal_wide_probe",
);

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
extern "C" {
    fn gal_user_call() -> u32;
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
pub fn call() -> u32 {
    unsafe { gal_user_call() }
}
