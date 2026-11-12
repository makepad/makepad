// A method call on an associated type or an opaque type sees only the traits in scope, as on any
// other type: a bound on the alias does not make its trait's methods callable. (chrono:
// `self.offset.fmt(f)` inside `impl fmt::Display for Date<Tz> where Tz::Offset: fmt::Display`,
// with `trait Offset: fmt::Debug` and `Debug` not imported, is `Display::fmt`, not an ambiguity.)
use core::fmt;

trait Offset: Sized + Clone + fmt::Debug {}
trait TimeZone {
    type Offset: Offset;
}
struct Date<Tz: TimeZone> {
    offset: Tz::Offset,
}
impl<Tz: TimeZone> fmt::Display for Date<Tz>
where
    Tz::Offset: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.offset.fmt(f)
    }
}

#[derive(Clone, Debug)]
struct Fixed(i32);
impl fmt::Display for Fixed {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "+{:02}:00", self.0)
    }
}
impl Offset for Fixed {}
struct Utc;
impl TimeZone for Utc {
    type Offset = Fixed;
}

#[test]
fn the_trait_in_scope_wins_over_a_bound_of_the_associated_type() {
    assert_eq!(format!("{}", Date::<Utc> { offset: Fixed(2) }), "+02:00");
}

// With the trait in scope, the bound still selects the impl and infers the trait's arguments.
trait Source {
    type Item: Into<u64> + Copy;
    fn get(&self) -> Self::Item;
}
struct Small(u8);
impl Source for Small {
    type Item = u8;
    fn get(&self) -> u8 { self.0 }
}
fn widen<S: Source>(s: &S) -> u64 {
    s.get().into()
}

mod shapes {
    pub trait Area { fn area(&self) -> u32; }
    pub struct Square(pub u32);
    impl Area for Square { fn area(&self) -> u32 { self.0 * self.0 } }
    pub fn unit() -> impl Area { Square(3) }
    pub fn evens() -> impl Iterator<Item = u32> + Clone { (0..10u32).filter(|n| n % 2 == 0) }
}

#[test]
fn a_bound_method_resolves_when_its_trait_is_in_scope() {
    use shapes::Area;
    assert_eq!(widen(&Small(7)), 7);
    assert_eq!(shapes::unit().area(), 9);
    assert_eq!(shapes::evens().clone().map(|n| n + 1).sum::<u32>(), 25);
}
