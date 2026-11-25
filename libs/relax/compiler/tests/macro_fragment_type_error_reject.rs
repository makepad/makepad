//! rustc reports the type mismatch at the string argument in the macro call.
// rejects: E0308
// rejects-at: 9:8 E0308
macro_rules! m {
    ($e:expr) => {{ let v: u32 = $e; v }};
}

pub fn run() -> u32 {
    m!("text")
}
