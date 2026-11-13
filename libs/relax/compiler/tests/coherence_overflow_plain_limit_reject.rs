// Coherence keeps the plain recursion limit: an overlap check whose goal overflows at the limit is
// ambiguous, so the impls conflict, though the goal fails at twice the limit (rustc 1.99.1, with
// or without -Znext-solver; the rerun is for type checking only:
// solver_overflow_twice_the_limit.rs).
// rejects: conflicting implementations of trait
#![recursion_limit = "32"]

pub struct Wrap<T>(pub T);

pub trait Plain {}
impl Plain for u8 {}
impl<T: Plain> Plain for Wrap<T> {}

pub trait Tagged {}
impl<T: Plain> Tagged for T {}
// 40 levels around u16, which is not `Plain`
impl Tagged for Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<u16>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>> {}
