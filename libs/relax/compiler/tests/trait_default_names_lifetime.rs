// A trait's defaulted type parameter that names the trait's own lifetime (`_Outlives = &'s Self`,
// the implied-bound idiom of reborrow traits): an impl header, a where-clause and a qualified path
// that leave it out instantiate the default with their own lifetime argument, as rustc does. Relax
// lowered the default with that lifetime erased: checking such impls panicked in region resolution
// ("non-concrete region in LUB"), and their callers were rejected (relax issue 240).

pub trait Reborrow<'s, _Outlives = &'s Self> {
    type Target;
    fn rb(&'s self) -> Self::Target;
}

pub struct View<'a, T> {
    data: &'a [T],
    stride: usize,
}
impl<T> Copy for View<'_, T> {}
impl<T> Clone for View<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'s, 'a, T> Reborrow<'s> for View<'a, T> {
    type Target = View<'s, T>;
    fn rb(&'s self) -> View<'s, T> {
        View { data: self.data, stride: self.stride }
    }
}

#[derive(Clone, Copy)]
pub struct Wrap<I>(pub I);
impl<'s, I: Reborrow<'s>> Reborrow<'s> for Wrap<I> {
    type Target = Wrap<I::Target>;
    fn rb(&'s self) -> Self::Target {
        Wrap(self.0.rb())
    }
}

pub struct Owned<T> {
    data: Vec<T>,
}
impl<'s, T> Reborrow<'s> for Owned<T> {
    type Target = View<'s, T>;
    fn rb(&'s self) -> View<'s, T> {
        View { data: &self.data, stride: 1 }
    }
}
impl<T> Owned<T> {
    pub fn as_view(&self) -> Wrap<View<'_, T>> {
        Wrap(self.rb())
    }
}

impl<'a, T> core::ops::Index<usize> for Wrap<View<'a, T>> {
    type Output = T;
    fn index(&self, i: usize) -> &T {
        let v = self.rb();
        &v.0.data[i * v.0.stride]
    }
}

pub fn first<'s, X: Reborrow<'s>>(x: &'s X) -> X::Target {
    x.rb()
}

pub fn target_of<'s, X: Reborrow<'s>>(x: &'s X) -> <X as Reborrow<'s>>::Target {
    x.rb()
}

#[test]
fn defaults_take_the_trait_refs_lifetime() {
    let o = Owned { data: vec![1u32, 2, 3, 4] };
    let w = o.as_view();
    assert_eq!(w[2], 3);
    let w2 = w.rb();
    assert_eq!(w2[0], 1);
    assert_eq!(first(&o).data.len(), 4);
    assert_eq!(target_of(&w).0.stride, 1);
}
