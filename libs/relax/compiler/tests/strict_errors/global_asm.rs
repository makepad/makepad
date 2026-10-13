//~ msg: inline assembly is not supported; use core::arch intrinsics
// global_asm! is rejected like asm!.
std::arch::global_asm!(".globl relax_test_sym", "relax_test_sym:", "ret");

fn main() {}
