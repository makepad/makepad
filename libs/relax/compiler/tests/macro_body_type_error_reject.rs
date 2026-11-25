//! The type mismatch points to the string literal written in the macro body.
// rejects: E0308
// rejects-at: 5:27 E0308
macro_rules! m {
    () => {{ let v: u32 = "text"; v }};
}

pub fn run() -> u32 {
    m!()
}
