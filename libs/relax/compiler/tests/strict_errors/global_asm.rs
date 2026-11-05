//~ msg: inline assembly (asm!, global_asm!, naked_asm!) in crate `global_asm` is blocked by `--no-asm` (MAKEPAD_RUN_NO_ASM=1)
//~ flags: --no-asm
std::arch::global_asm!(".globl relax_test_sym", "relax_test_sym:", "ret");

fn main() {}
