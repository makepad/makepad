// rejects: E0432
// rejects: unresolved import `core::intrinsics_mem`
// rejects-at: 4:11 E0432
use core::intrinsics_mem::is_copy;
pub fn f() -> bool { unsafe { is_copy::<u8>() } }
