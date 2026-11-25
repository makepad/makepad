//! The call ends after the comma where the matcher expects another expression: rustc reports it
//! just after the comma, not at the closing delimiter.
// rejects: unexpected end of macro invocation
// rejects-at: 10:10 unexpected end of macro invocation
macro_rules! m {
    ($a:expr, $b:expr) => {};
}

pub fn run() {
    m!(1, );
}
