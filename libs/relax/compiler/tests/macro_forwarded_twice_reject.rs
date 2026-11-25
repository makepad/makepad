//! An expression forwarded twice stays opaque to the innermost identifier matcher: rustc reports
//! it at the `$e` that forwards it last.
// rejects: no rules expected `expr` metavariable
// rejects-at: 9:27 no rules expected `expr` metavariable
macro_rules! inner {
    ($i:ident) => { 0 };
}
macro_rules! mid {
    ($e:expr) => { inner!($e) };
}
macro_rules! outer {
    ($e:expr) => { mid!($e) };
}

pub fn run() -> u32 {
    outer!(x)
}
