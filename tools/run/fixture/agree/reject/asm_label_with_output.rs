// rejects: E0658
pub unsafe fn f() -> u64 { let x: u64; unsafe { core::arch::asm!("mov {0}, 1", "jmp {1}", out(reg) x, label {}); } x }
