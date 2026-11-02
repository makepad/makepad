// `label` operands of asm! (stable asm goto): a jump runs the label's block, falling through skips it.
#[cfg(target_arch = "x86_64")]
fn jump(taken: u64) -> u32 {
    let mut seen = 0;
    unsafe {
        core::arch::asm!("test {0}, {0}", "jne {1}", "jmp {2}", in(reg) taken, label { seen += 1; }, label { seen += 10; });
    }
    seen
}
#[cfg(target_arch = "aarch64")]
fn jump(taken: u64) -> u32 {
    let mut seen = 0;
    unsafe {
        core::arch::asm!("cbnz {0}, {1}", "b {2}", in(reg) taken, label { seen += 1; }, label { seen += 10; });
    }
    seen
}
#[cfg(target_arch = "x86_64")]
fn falls_through() -> u32 {
    let mut seen = 0;
    unsafe { core::arch::asm!("nop", "jmp 2f", "jmp {}", "2:", label { seen += 1; }); }
    seen + 5
}
#[cfg(target_arch = "aarch64")]
fn falls_through() -> u32 {
    let mut seen = 0;
    unsafe { core::arch::asm!("nop", "b 2f", "b {}", "2:", label { seen += 1; }); }
    seen + 5
}
fn returns() -> u32 {
    #[cfg(target_arch = "x86_64")]
    unsafe { core::arch::asm!("jmp {}", label { return 7; }); }
    #[cfg(target_arch = "aarch64")]
    unsafe { core::arch::asm!("b {}", label { return 7; }); }
    #[allow(unreachable_code)]
    0
}
#[test]
fn labels() {
    assert_eq!(jump(1), 1);
    assert_eq!(jump(0), 10);
    assert_eq!(falls_through(), 5);
    assert_eq!(returns(), 7);
}
