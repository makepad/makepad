// rejects: E0433
// rejects: cannot find `intrinsics_mem` in `core`
// rejects-at: 5:37 E0433
// (a fn no test calls: the test forms load its body lazily, and the gates still see it)
pub fn f() -> bool { unsafe { core::intrinsics_mem::is_copy::<u8>() } }
