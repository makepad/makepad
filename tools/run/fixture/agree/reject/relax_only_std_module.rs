// rejects: E0658
use core::intrinsics_mem::is_copy;
pub fn f() -> bool { unsafe { is_copy::<u8>() } }
