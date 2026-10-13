//~ msg: inline assembly is not supported; use core::arch intrinsics
// Relax has no assembler (user decision 2026-10-05): asm! is a compile error naming the
// alternative. (rustc accepts this on aarch64; Relax rejects it on every target.)
fn read() -> u64 {
    let v: u64;
    unsafe { std::arch::asm!("mov {0}, #7", out(reg) v) };
    v
}

fn main() {
    println!("{}", read());
}
