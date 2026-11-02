// rejects: E0308
pub unsafe fn f() { unsafe { core::arch::asm!("jmp {}", label { 1u8 }); } }
