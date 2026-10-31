trait ConstMethod { fn value<const N: usize>() -> usize; }
impl<'a> ConstMethod for &'a u8 { fn value<const N: usize>() -> usize { N } }

#[test]
fn method_const_follows_impl_lifetime() {
    assert_eq!(<&u8 as ConstMethod>::value::<7>(), 7);
}

trait Iter { type Item; }
trait Outer { type A: Iter<Item: Copy>; }
struct S;
impl Iter for S { type Item = u32; }
impl Outer for S { type A = S; }
fn nested<T: Outer>(value: <<T as Outer>::A as Iter>::Item) -> <<T as Outer>::A as Iter>::Item {
    let copied = value;
    let _also_copied = value;
    copied
}
#[test]
fn nested_associated_item_bounds() { assert_eq!(nested::<S>(9), 9); }

fn unused_binder<T: for<'a> Fn() -> u32>(f: T) -> u32 { f() }
#[test]
fn unused_higher_ranked_lifetime() { assert_eq!(unused_binder(|| 11), 11); }

trait Project<'a> { type Out; }
impl<'a> Project<'a> for u32 { type Out = &'a u32; }
fn project<'a>(value: &'a u32) -> <u32 as Project<'a>>::Out { value }
#[test]
fn projection_preserves_lifetime() { let value = 13; assert_eq!(*project(&value), value); }
