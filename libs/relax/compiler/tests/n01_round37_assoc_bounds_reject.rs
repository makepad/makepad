// rustc_hir_analysis/src/check/compare_impl_item.rs:2320-2597.
trait Mark {}
trait Family { type Member<T>: Mark; }
struct Bad;
impl Family for Bad { type Member<T> = T; }
