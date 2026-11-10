// A tuple index names a field of a tuple struct only: on a struct with named fields `x.0` is a
// field of its Deref target (ruma-events: `self.0.try_into()` on an owned identifier, whose own
// field is `inner`, reads the `str` of the borrowed identifier it derefs to).
use std::ops::Deref;

#[repr(transparent)]
struct Id(str);
impl Id {
    fn from_str(s: &str) -> &Id { unsafe { &*(s as *const str as *const Id) } }
}

// named fields: `.0` is not one of them, so `owned.0` is `Id`'s field, reached through Deref
struct OwnedId { inner: Box<str> }
impl Deref for OwnedId {
    type Target = Id;
    fn deref(&self) -> &Id { Id::from_str(&self.inner) }
}

struct Pair(u8, u16);
struct Wrapper { first: u64, second: Pair }
impl Deref for Wrapper {
    type Target = Pair;
    fn deref(&self) -> &Pair { &self.second }
}

#[test]
fn a_tuple_index_skips_a_struct_with_named_fields() {
    let owned = OwnedId { inner: "@a:b".into() };
    let s: &str = &owned.0;
    assert_eq!(s, "@a:b");
    assert!(owned.0.eq("@a:b"));
    let r: Result<&str, _> = owned.0.try_into();
    let _: std::convert::Infallible = match r { Ok(s) => { assert_eq!(s.len(), 4); return; } Err(e) => e };
}

#[test]
fn a_tuple_index_reaches_the_deref_target() {
    let w = Wrapper { first: 9, second: Pair(1, 2) };
    assert_eq!(w.first, 9);
    assert_eq!(w.0, 1u8);
    assert_eq!(w.1, 2u16);
}
