//! A forwarded `expr` fragment is one opaque expression: an `ident` matcher does not take it
//! even when it is one identifier (rustc's matcher; relax issue 21).
// rejects: no rule of `want_ident!` matches these arguments
macro_rules! want_ident {
    ($i:ident) => { 0 };
}
macro_rules! forward {
    ($e:expr) => { want_ident!($e) };
}

pub fn opaque() {
    assert_eq!(forward!(x), 0);
}
