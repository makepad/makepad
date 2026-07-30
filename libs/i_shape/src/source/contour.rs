use crate::source::resource::ShapeResource;
use alloc::vec::Vec;
use i_float::float::compatible::FloatPointCompatible;

pub struct ContourResourceIterator<'a, P> {
    slice: &'a [P],
    finished: bool,
}

impl<'a, P> ContourResourceIterator<'a, P> {
    #[inline]
    fn with_slice(slice: &'a [P]) -> Self {
        Self {
            slice,
            finished: false,
        }
    }
}

impl<'a, P> Iterator for ContourResourceIterator<'a, P> {
    type Item = &'a [P];

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        self.finished = true;
        Some(self.slice)
    }

    #[inline]
    fn count(self) -> usize
    where
        Self: Sized,
    {
        1
    }
}

impl<P> ShapeResource<P> for [P]
where
    P: FloatPointCompatible,
{
    type ResourceIter<'a>
        = ContourResourceIterator<'a, P>
    where
        P: 'a,
        Self: 'a;

    #[inline]
    fn iter_paths(&self) -> Self::ResourceIter<'_> {
        ContourResourceIterator::with_slice(self)
    }
}

impl<P, const N: usize> ShapeResource<P> for [P; N]
where
    P: FloatPointCompatible,
{
    type ResourceIter<'a>
        = ContourResourceIterator<'a, P>
    where
        P: 'a,
        Self: 'a;

    #[inline]
    fn iter_paths(&self) -> Self::ResourceIter<'_> {
        ContourResourceIterator::with_slice(self)
    }
}

impl<P> ShapeResource<P> for Vec<P>
where
    P: FloatPointCompatible,
{
    type ResourceIter<'a>
        = ContourResourceIterator<'a, P>
    where
        P: 'a,
        Self: 'a;

    #[inline]
    fn iter_paths(&self) -> Self::ResourceIter<'_> {
        ContourResourceIterator::with_slice(self.as_slice())
    }
}

impl<'b, P> ShapeResource<P> for &'b [P]
where
    P: FloatPointCompatible,
{
    type ResourceIter<'a>
        = ContourResourceIterator<'a, P>
    where
        P: 'a,
        Self: 'a;

    #[inline]
    fn iter_paths(&self) -> Self::ResourceIter<'b> {
        ContourResourceIterator::with_slice(self)
    }
}

#[cfg(test)]
mod tests {
    use crate::source::resource::ShapeResource;
    use alloc::vec;

    #[test]
    fn test_resource_fixed_array() {
        let array = [[0.0, 0.0], [0.0, 1.0]];

        let count = array.iter_paths().fold(0, |s, it| s + it.len());

        assert_eq!(count, 2);
    }

    #[test]
    fn test_resource_slice_array() {
        let array = [[0.0, 0.0], [0.0, 1.0]];

        let count = array.as_slice().iter_paths().fold(0, |s, it| s + it.len());

        assert_eq!(count, 2);
    }

    #[test]
    fn test_resource_vec_array() {
        let array = vec![[0.0, 0.0], [0.0, 1.0]];

        let count = array.iter_paths().fold(0, |s, it| s + it.len());

        assert_eq!(count, 2);
    }
}
