// T4: stable asm goto label operands; owner: codegen/backend.
#[test]
#[ignore = "T4: asm goto label operands; codegen/backend"]
fn asm_label_operand() {
    unsafe { core::arch::asm!("jmp {}", label { return; }); }
    panic!("asm branch did not reach its label");
}
