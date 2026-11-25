//! A single-literal matcher refuses the second literal: rustc reports it at that `2`, not at the call.
// rejects: no rules expected
// rejects-at: 9:10 no rules expected
macro_rules! m {
    ($a:literal) => {};
}

pub fn run() {
    m!(1 2);
}
