// A 2021 method token keeps its macro-definition edition in a 2018 crate: rustc accepts
// (rustc_span/src/hygiene.rs:922-924; rustc_hir_typeck/src/method/probe.rs:2018-2035).
// Run as a package: makepad check --no-serve n01-round37-macro-edition.
fn consume<I: Iterator<Item = u8>>(_: I) {}
fn main() { consume(new_macros::by_value!([1u8])); }
