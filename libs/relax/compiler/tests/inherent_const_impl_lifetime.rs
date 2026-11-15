// An inherent associated const of a type with a lifetime parameter, named from a body with generic
// parameters of its own: a trait's default method (its Self) and a generic fn (relax issue 188:
// "wrong generic argument kind" when that body was compiled for its arguments). The impl's
// lifetime stayed in the const's type as an early-bound region, and the calling body's arguments,
// a type in that slot, replaced it.
pub struct Holder<'a, T> {
    r: &'a T,
}

impl Holder<'_, ()> {
    const EMPTY: Self = Self { r: &() };
}

impl<'a> Holder<'a, u16> {
    const TWO: Holder<'a, u16> = Holder { r: &2 };
}

fn width(h: Holder<()>) -> usize {
    std::mem::size_of_val(h.r) + 3
}

pub trait Measured {
    fn measured(&self) -> usize {
        width(Holder::EMPTY)
    }
}

impl Measured for u8 {}

fn generic<T: Copy>(t: T) -> (T, usize, u16) {
    (t, width(Holder::EMPTY), *Holder::TWO.r)
}

#[test]
fn an_inherent_const_with_an_impl_lifetime_is_named_from_generic_bodies() {
    assert_eq!(7u8.measured(), 3, "from a trait's default method");
    assert_eq!(generic(5u32), (5, 3, 2), "from a generic fn, with a named impl lifetime too");
}
