// A proc-macro-hack entry point is imported by its derive name, which only the expansion of the
// implementation crate's `#[proc_macro_hack]` defines: the scan of that crate's source names it
// (before: pmh-decl's import was unresolved, a load error). And a proc macro runs with CARGO set,
// as under cargo, also when the run did not come from cargo.
use pmh_decl as _;

fn main() {
    println!("hack entry ok, CARGO set: {}", pmh_hack::cargo_set!());
}
