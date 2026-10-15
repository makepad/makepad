// P3/H4 OPEN, front + arch. This package is accepted by rustc and rejected by
// Relax. A 2021 method token keeps its macro-definition edition in a 2018 crate.
// rustc_span/src/hygiene.rs:922-924; rustc_hir_typeck/src/method/probe.rs:2087.
// Relax HygCtxt/rules_exp carries expansion marks without ExpnData.edition.
// Run as a package: makepad check --no-serve n01-round37-macro-edition.
fn consume<I: Iterator<Item = u8>>(_: I) {}
fn main() { consume(new_macros::by_value!([1u8])); }
