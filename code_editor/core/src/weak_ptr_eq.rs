use std::{
    hash::{Hash, Hasher},
    rc::Weak,
};

#[derive(Clone, Debug)]
pub struct WeakPtrEq<T>(pub Weak<T>);

impl<T> PartialEq for WeakPtrEq<T> {
    fn eq(&self, other: &WeakPtrEq<T>) -> bool {
        Weak::ptr_eq(&self.0, &other.0)
    }
}

impl<T> Eq for WeakPtrEq<T> {}

impl<T> Hash for WeakPtrEq<T> {
    fn hash<H>(&self, hasher: &mut H)
    where
        H: Hasher,
    {
        hasher.write_usize(Weak::as_ptr(&self.0) as usize);
    }
}
