//~ msg: inline assembly (asm!, global_asm!, naked_asm!) in crate `inline_asm` is blocked by `--no-asm` (RELAX_NO_ASM=1)
//~ flags: --no-asm
fn read() -> u64 {
    let v: u64;
    unsafe { std::arch::asm!("mov {0}, #7", out(reg) v) };
    v
}

fn main() {
    println!("{}", read());
}
