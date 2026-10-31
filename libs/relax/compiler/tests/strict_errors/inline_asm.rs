//~ msg: inline assembly (asm!, global_asm!, naked_asm!) in crate `test_crate` is blocked by `--no-asm` (MAKEPAD_RUN_NO_ASM=1)
// Run with --no-asm (MAKEPAD_RUN_NO_ASM=1): asm! is then a compile error naming the crate and the flag.
fn read() -> u64 {
    let v: u64;
    unsafe { std::arch::asm!("mov {0}, #7", out(reg) v) };
    v
}

fn main() {
    println!("{}", read());
}
