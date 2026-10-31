// D7 / front-bck audit 1.5: rustc rejects this (E0310): a rigid projection keeps its type test in
// borrowck (nll.rs params_in matches Alias(_, Projection)). Checked as a package with
// `makepad check` (borrowck runs on every body there).
#![allow(dead_code)]
trait Tr { type Item; }
fn need_static<U: 'static>(_: U) {}
fn bad<T: Tr>(x: T::Item) { need_static(x); }
