// A library's `main` is a function like any other: no entry point rules apply.
pub fn main<T>(_: T) {}
#[test] fn runs() { main(1); }
