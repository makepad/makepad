use crate::{InternalIterator, IntoInternalIterator};

pub trait FromInternalIterator<T> {
    fn from_internal_iter<I: IntoInternalIterator<Item = T>>(iter: I) -> Self;
}

impl<T> FromInternalIterator<T> for Vec<T> {
    fn from_internal_iter<I: IntoInternalIterator<Item = T>>(iter: I) -> Self {
        let mut vec = Vec::new();
        iter.into_internal_iter().for_each(&mut |item| {
            vec.push(item);
            true
        });
        vec
    }
}
