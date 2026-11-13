// A method call on an associated-type (projection) receiver takes its candidates only from the traits
// in scope, never from the type's item bounds or from where-clauses on it (rustc's probe; relax issue
// 101). `self.offset.fmt(f)` in a Display impl is Display's although the associated type's bound has
// Debug as a supertrait, and a bound trait that is not in scope leaves the one that is.
use std::fmt;

pub trait Offset: Clone + fmt::Debug {
    fn fix(&self) -> i32;
}
pub trait TimeZone: Sized + Clone {
    type Offset: Offset;
}
#[derive(Clone, Debug)]
pub struct Utc;
#[derive(Clone, Debug)]
pub struct UtcOffset;
impl Offset for UtcOffset {
    fn fix(&self) -> i32 {
        0
    }
}
impl fmt::Display for UtcOffset {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "UTC")
    }
}
impl TimeZone for Utc {
    type Offset = UtcOffset;
}

pub struct Date<Tz: TimeZone> {
    offset: Tz::Offset,
    day: u32,
}
impl<Tz: TimeZone> fmt::Display for Date<Tz>
where
    Tz::Offset: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.day.fmt(f)?;
        self.offset.fmt(f)
    }
}
impl<Tz: TimeZone> fmt::Debug for Date<Tz> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.day.fmt(f)?;
        self.offset.fmt(f)
    }
}

mod picks {
    pub trait Bound {
        fn pick(&self) -> u32 {
            1
        }
    }
    pub trait Scoped {
        fn pick(&self) -> u32 {
            2
        }
    }
    impl Bound for u8 {}
    impl Scoped for u8 {}
}
use picks::Scoped;
pub trait Holder {
    type Item: picks::Bound;
}
impl Holder for () {
    type Item = u8;
}
pub fn pick<T: Holder>(item: T::Item) -> u32
where
    T::Item: Scoped,
{
    item.pick()
}

#[test]
fn projection_receivers_use_the_traits_in_scope() {
    let d = Date::<Utc> { offset: UtcOffset, day: 7 };
    assert_eq!(format!("{} {:?}", d, d), "7UTC 7UtcOffset", "Display and Debug of the offset");
    assert_eq!(UtcOffset.fix(), 0);
    assert_eq!(pick::<()>(0), 2, "the bound trait is not in scope");
}
