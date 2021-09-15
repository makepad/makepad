use std::{
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
};

pub struct Id<T> {
    pub(super) index: usize,
    pub(super) generation: usize,
    pub(super) phantom: PhantomData<T>,
}

impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        Self {
            index: 0,
            generation: 0,
            phantom: PhantomData,
        }
    }
}

impl<T> Copy for Id<T> {}

impl<T> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Id")
            .field("index", &self.index)
            .field("generation", &self.generation)
            .finish()
    }
}

impl<T> Eq for Id<T> {}

impl<T> Hash for Id<T> {
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        self.index.hash(state);
        self.generation.hash(state);
    }
}

impl<T> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        if self.index != other.index {
            return false;
        }
        if self.generation != other.generation {
            return false;
        }
        true
    }
}
