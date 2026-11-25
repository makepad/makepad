// A field whose type is an associated-type projection: the struct's inferred outlives bounds come
// from the projection's arguments (`'v: 'a` through `<&'a [Entry<'v>] as IntoIterator>::IntoIter`)
// and from the trait's own outlives clauses (`U: 'a` through `<W as Window<'a, U>>::View`), as
// rustc infers them, and the struct's uses assume them.
pub struct Entry<'v> {
    pub key: &'v str,
}
pub struct Cursor<'a, 'v> {
    pub inner: <&'a [Entry<'v>] as IntoIterator>::IntoIter,
}
impl<'a, 'v> Cursor<'a, 'v> {
    // `&'v str` as `&'a str` needs `'v: 'a`, which the struct implies
    pub fn next_key(&mut self) -> Option<&'a str> {
        self.inner.next().map(|e| e.key)
    }
}
pub trait Window<'a, U: 'a> {
    type View;
    fn view(u: &'a U) -> Self::View;
}
pub struct Whole;
impl<'a, U: 'a> Window<'a, U> for Whole {
    type View = &'a U;
    fn view(u: &'a U) -> &'a U {
        u
    }
}
pub struct Held<'a, U, W: Window<'a, U>> {
    pub view: <W as Window<'a, U>>::View,
}
fn outlives<'a, U: 'a>() -> bool {
    true
}
pub fn holds<'a, U, W: Window<'a, U>>(_h: &Held<'a, U, W>) -> bool {
    outlives::<'a, U>()
}
#[test]
fn runs() {
    let entries = [Entry { key: "x" }, Entry { key: "yz" }];
    let mut c = Cursor { inner: entries.iter() };
    assert_eq!(c.next_key(), Some("x"));
    assert_eq!(c.next_key(), Some("yz"));
    assert_eq!(c.next_key(), None);
    let n = 3u8;
    let h: Held<'_, u8, Whole> = Held { view: Whole::view(&n) };
    assert_eq!(*h.view, 3);
    assert!(holds(&h));
}
