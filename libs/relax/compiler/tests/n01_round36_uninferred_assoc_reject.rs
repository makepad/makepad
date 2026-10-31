//~ E0212
// rustc_hir_analysis/src/collect.rs:627: an associated item path in an
// item type cannot discard the trait bound's uninferred lifetime binder.
trait At<'a> { type Value; }
fn invalid<T: for<'a> At<'a>>(_: T::Value) {}
fn main() {}
