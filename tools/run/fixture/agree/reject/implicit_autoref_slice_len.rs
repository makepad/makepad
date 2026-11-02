// rejects: implicit autoref creates a reference to the dereference of a raw pointer
pub unsafe fn f(raw: *const [i32]) -> usize { unsafe { (*raw).len() } }
