// A proc-macro-hack entry point is imported by its derive name, which only the expansion of the
// implementation crate's `#[proc_macro_hack]` defines: the scan of that crate's source names it
// (before: pmh-decl's import was unresolved, a load error). A proc macro runs with CARGO set, as
// under cargo, also when the run did not come from cargo. And the `macro_rules!` the derive writes
// inside pmh-decl's `answer!` takes `$crate` from there: pmh-decl, not this crate (relax PR 84:
// `unic_langid::langid!` failed with E0433 on `__relax_crate_<caller>::subtags::Language`).
fn main() {
    println!("hack entry ok, CARGO set: {}, answer {}", pmh_hack::cargo_set!(), pmh_decl::answer!(the question));
}
