use crate::source::resource::ShapeResource;
use alloc::vec::Vec;
use i_float::float::compatible::FloatPointCompatible;

pub struct ShapesResourceIterator<'a, P> {
    slice: &'a [Vec<Vec<P>>],
    i: usize,
    j: usize,
}

impl<'a, P> ShapesResourceIterator<'a, P> {
    #[inline]
    fn with_slice(slice: &'a [Vec<Vec<P>>]) -> Self {
        Self { slice, i: 0, j: 0 }
    }
}

impl<'a, P> Iterator for ShapesResourceIterator<'a, P> {
    type Item = &'a [P];

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        while self.i < self.slice.len() {
            let sub_slice = unsafe { self.slice.get_unchecked(self.i) };
            if self.j < sub_slice.len() {
                let j = self.j;
                self.j += 1;
                let it = unsafe { sub_slice.get_unchecked(j) };
                return Some(it.as_slice());
            }
            self.i += 1;
            self.j = 0;
        }

        None
    }

    #[inline]
    fn count(self) -> usize
    where
        Self: Sized,
    {
        self.slice.iter().fold(0, |s, shape| s + shape.len())
    }
}

impl<P> ShapeResource<P> for [Vec<Vec<P>>]
where
    P: FloatPointCompatible,
{
    type ResourceIter<'a>
        = ShapesResourceIterator<'a, P>
    where
        P: 'a,
        Self: 'a;

    #[inline]
    fn iter_paths(&self) -> Self::ResourceIter<'_> {
        ShapesResourceIterator::with_slice(self)
    }
}

impl<P, const N: usize> ShapeResource<P> for [Vec<Vec<P>>; N]
where
    P: FloatPointCompatible,
{
    type ResourceIter<'a>
        = ShapesResourceIterator<'a, P>
    where
        P: 'a,
        Self: 'a;

    #[inline]
    fn iter_paths(&self) -> Self::ResourceIter<'_> {
        ShapesResourceIterator::with_slice(self)
    }
}

impl<P> ShapeResource<P> for Vec<Vec<Vec<P>>>
where
    P: FloatPointCompatible,
{
    type ResourceIter<'a>
        = ShapesResourceIterator<'a, P>
    where
        P: 'a,
        Self: 'a;

    #[inline]
    fn iter_paths(&self) -> Self::ResourceIter<'_> {
        ShapesResourceIterator::with_slice(self.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use crate::source::resource::ShapeResource;
    use alloc::vec;

    #[test]
    fn test_resource_fixed_array() {
        let array = [vec![vec![[0.0, 0.0], [0.0, 1.0]]]];

        let count = array.iter_paths().fold(0, |s, it| s + it.len());

        assert_eq!(count, 2);
    }

    #[test]
    fn test_resource_slice_array() {
        let array = [vec![vec![[0.0, 0.0], [0.0, 1.0]]]];

        let count = array.as_slice().iter_paths().fold(0, |s, it| s + it.len());

        assert_eq!(count, 2);
    }

    #[test]
    fn test_resource_vec_array() {
        let array = vec![vec![vec![[0.0, 0.0], [0.0, 1.0]]]];

        let count = array.iter_paths().fold(0, |s, it| s + it.len());

        assert_eq!(count, 2);
    }
}
