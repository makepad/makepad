// rejects: E0133
pub unsafe fn g() {}
pub unsafe fn f() { unsafe { core::arch::asm!("jmp {}", label { g(); }); } }
