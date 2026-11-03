// Stable asm goto: a `label` operand whose block returns; the jump to it skips the code after the asm!.
// needs-native: inline assembly
#[test]
fn asm_label_operand() {
    #[cfg(target_arch = "x86_64")]
    unsafe { core::arch::asm!("jmp {}", label { return; }); }
    #[cfg(target_arch = "aarch64")]
    unsafe { core::arch::asm!("b {}", label { return; }); }
    panic!("asm branch did not reach its label");
}
