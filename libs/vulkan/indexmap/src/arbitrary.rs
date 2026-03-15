#[cfg(feature = "arbitrary")]
#[cfg_attr(docsrs, doc(cfg(feature = "arbitrary")))]
mod impl_arbitrary {
    use crate::{IndexMap, IndexSet};
    use arbitrary::{Arbitrary, Result, Unstructured};
    use core::hash::{BuildHasher, Hash};

    impl<'a, K, V, S> Arbitrary<'a> for IndexMap<K, V, S>
    where
        K: Arbitrary<'a> + Hash + Eq,
        V: Arbitrary<'a>,
        S: BuildHasher + Default,
    {
        fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
            u.arbitrary_iter()?.collect()
        }

        fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
            u.arbitrary_take_rest_iter()?.collect()
        }
    }

    impl<'a, T, S> Arbitrary<'a> for IndexSet<T, S>
    where
        T: Arbitrary<'a> + Hash + Eq,
        S: BuildHasher + Default,
    {
        fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
            u.arbitrary_iter()?.collect()
        }

        fn arbitrary_take_rest(u: Unstructured<'a>) -> Result<Self> {
            u.arbitrary_take_rest_iter()?.collect()
        }
    }
}
