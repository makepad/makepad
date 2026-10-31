// rustc_hir_analysis/src/check/compare_impl_item.rs:2320-2757.
trait Mark { fn value() -> u32 { 8 } }
struct Member<T>(T);
impl<T> Mark for Member<T> {}
trait Family { type Item<T>: Mark; }
struct Good;
impl Family for Good { type Item<T> = Member<T>; }
fn get<T: Family>() -> u32 { <T::Item<()> as Mark>::value() }
#[test]
fn gat_item_bounds() { assert_eq!(get::<Good>(), 8); }
