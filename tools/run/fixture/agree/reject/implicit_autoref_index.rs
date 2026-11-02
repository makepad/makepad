// rejects: implicit autoref creates a reference to the dereference of a raw pointer
pub unsafe fn f(raw: *mut [u8]) -> *mut [u8] { unsafe { &raw mut (*raw)[..16] } }
