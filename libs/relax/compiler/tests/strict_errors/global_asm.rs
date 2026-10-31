//~ msg: inline assembly (asm!, global_asm!, naked_asm!) in crate `test_crate` is blocked by `--no-asm` (MAKEPAD_RUN_NO_ASM=1)
// Run with --no-asm (MAKEPAD_RUN_NO_ASM=1): global_asm! is rejected like asm!.
std::arch::global_asm!(".globl relax_test_sym", "relax_test_sym:", "ret");

fn main() {}
