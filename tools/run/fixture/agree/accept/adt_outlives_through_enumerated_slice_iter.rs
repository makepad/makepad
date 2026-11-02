// `T: 'a` is inferred for the struct through `Enumerate<Iter<'a, Option<T>>>`, and its impls assume it.
use core::marker::PhantomData;
#[derive(Debug, Clone)]
pub struct ArenaIter<'a, I, T> {
    inner: core::iter::Enumerate<core::slice::Iter<'a, Option<T>>>,
    _id: PhantomData<fn() -> I>,
}
impl<'a, I: From<u32>, T> Iterator for ArenaIter<'a, I, T> {
    type Item = (I, &'a T);
    fn next(&mut self) -> Option<Self::Item> {
        for (i, slot) in self.inner.by_ref() {
            if let Some(t) = slot { return Some((I::from(i as u32), t)); }
        }
        None
    }
}
#[test] fn runs() {
    let items = [Some(1), None, Some(3)];
    let it: ArenaIter<'_, u32, i32> = ArenaIter { inner: items.iter().enumerate(), _id: PhantomData };
    assert_eq!(it.map(|(_, t)| *t).sum::<i32>(), 4);
}
