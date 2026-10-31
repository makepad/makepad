// rustc_parse parser/mod.rs: break_and_eat/eat_lt/expect_gt split glued
// operators only when the grammar requests one half, including in fragments.
trait Tr { type A; }
impl Tr for () { type A = u32; }
impl Tr for u32 { type A = u8; }

macro_rules! m {
    ($t:ty) => { struct S { f: $t } };
}
m!(Option<<() as Tr>::A>);

macro_rules! alias {
    ($name:ident, $p:path) => { type $name = $p; };
}
alias!(P, Option<<() as Tr>::A>);

macro_rules! typed {
    ($t:ty = $e:expr) => {{ let value: $t = $e; value }};
}

fn plain<T: Tr>(value: T::A) -> Option<<T as Tr>::A> { Some(value) }
fn identity<T>(value: T) -> T { value }
struct Signed<const N: i32>;
fn signed<const N: i32>(_: Signed<N>) -> i32 { N }

trait Generic { type Item<T>; }
impl Generic for () { type Item<T> = T; }
fn associated<T: Generic<Item<<() as Tr>::A> = u32>>(_: T) -> u32 { 18 }
fn associated_nested<T: Generic<Item<Option<u32>>=Option<u32>>>(_: T) -> u32 { 19 }

#[test]
fn qualified_types_in_fragments_and_plain_source() {
    let s = S { f: Some(7) };
    let p: P = plain::<()>(8);
    let nested: Option<<u32 as Tr>::A> = Some(9);
    let qself = typed!(<<() as Tr>::A as Tr>::A = 10);
    assert_eq!(s.f, Some(7));
    assert_eq!(p, Some(8));
    assert_eq!(nested, Some(9));
    assert_eq!(qself, 10u8);
    assert_eq!(identity::<Option<<() as Tr>::A>>(Some(11)), Some(11));
    assert_eq!(associated(()), 18);
    assert_eq!(associated_nested(()), 19);
}

#[test]
fn glued_closing_angles_and_negative_const_arguments() {
    assert_eq!(typed!(Option<u32>=Some(12)), Some(12));
    assert_eq!(typed!(Option<Option<u32>>=Some(Some(13))), Some(Some(13)));
    let one: Option<u32>=Some(14);
    let two: Option<Option<u32>>=Some(Some(15));
    assert_eq!(one, Some(14));
    assert_eq!(two, Some(Some(15)));
    assert_eq!(signed(typed!(Signed<-1> = Signed)), -1);
    assert_eq!(signed(Signed::<-2>), -2);
}

#[test]
fn other_grammar_splits_keep_the_captured_tokens() {
    let value = 16;
    let reference = &value;
    assert_eq!(**typed!(&&u32 = &reference), 16);
    assert_eq!(typed!(u32 = **&&value), 16);
    let closure = typed!(fn() -> u32 = || 17);
    assert_eq!(closure(), 17);
}
