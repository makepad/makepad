extern "C" {
    fn shim_answer() -> i32;
}

/// The shim's answer, which it asks the shared library for.
pub fn answer() -> i32 {
    unsafe { shim_answer() }
}
