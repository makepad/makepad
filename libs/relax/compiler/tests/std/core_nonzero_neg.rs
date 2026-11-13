// Neg for the signed NonZero types and for references to them (stable since 1.71). ctutils 0.4's
// `impl_signed_ct_neg!` (src/traits/ct_neg.rs) negates `NonZeroI8` .. `NonZeroIsize` with
// `self.ct_select(&-*self, choice)`; the std here had `wrapping_neg` and friends but no operator.
use std::num::{NonZero, NonZeroI128, NonZeroI16, NonZeroI32, NonZeroI64, NonZeroI8, NonZeroIsize};
use std::ops::Neg;

// ctutils' shape: the operator on a deref of `&self`, the result borrowed again
trait CtNeg: Sized {
    fn ct_neg(&self, choice: bool) -> Self;
}
macro_rules! impl_signed_ct_neg {
    ($($int:ty),+) => {
        $(impl CtNeg for $int {
            fn ct_neg(&self, choice: bool) -> Self {
                let negated = &-*self;
                if choice { *negated } else { *self }
            }
        })+
    };
}
impl_signed_ct_neg!(i8, i16, i32, i64, i128, isize, NonZeroI8, NonZeroI16, NonZeroI32, NonZeroI64, NonZeroI128, NonZeroIsize);

fn negate<T: Neg<Output = T>>(t: T) -> T {
    -t
}
fn negate_ref<'a, T>(t: &'a T) -> T
where
    &'a T: Neg<Output = T>,
{
    -t
}

#[test]
fn every_signed_nonzero_negates() {
    macro_rules! each {
        ($($nz:ident $int:ident),+) => {
            $(
                let five = $nz::new(5).unwrap();
                let minus = -five;
                assert_eq!(minus.get(), -5);
                assert_eq!(-minus, five);
                assert_eq!((-&five).get(), -5);
                assert_eq!(negate(five).get(), -5);
                assert_eq!(negate_ref(&minus).get(), 5);
                assert_eq!(five.ct_neg(true).get(), -5);
                assert_eq!(five.ct_neg(false).get(), 5);
                // the far ends: MAX and MIN + 1 are each other's negation
                let max = $nz::new($int::MAX).unwrap();
                assert_eq!((-max).get(), $int::MIN + 1);
                assert_eq!(-(-max), $nz::MAX);
                assert_eq!(std::mem::size_of_val(&Some(-max)), std::mem::size_of::<$int>());
            )+
        };
    }
    each!(NonZeroI8 i8, NonZeroI16 i16, NonZeroI32 i32, NonZeroI64 i64, NonZeroI128 i128, NonZeroIsize isize);
}

#[test]
fn nonzero_neg_through_the_generic_name() {
    let n: NonZero<i32> = NonZero::new(-40).unwrap();
    let m: NonZero<i32> = -n;
    assert_eq!(format!("{:?} {:?} {}", n, m, -&m), "-40 40 -40");
    let all: Vec<i64> = [3i64, -7, 1].iter().map(|&v| (-NonZero::new(v).unwrap()).get()).collect();
    assert_eq!(all, [-3, 7, -1]);
    assert_eq!(<NonZero<i16> as Neg>::neg(NonZero::new(9).unwrap()).get(), -9);
}
