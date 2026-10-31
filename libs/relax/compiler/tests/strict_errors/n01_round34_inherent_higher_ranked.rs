// Inherent overlap deliberately skips the leak check (inherent_impls_overlap.rs:162-175).
struct S<T>(T);
impl<T> S<fn(T)> { fn same() {} }
impl<T> S<for<'a> fn(&'a T)> { fn same() {} }
fn main() {}
