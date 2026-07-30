use i_float::float::compatible::FloatPointCompatible;

pub trait ShapeResource<P>
where
    P: FloatPointCompatible,
{
    type ResourceIter<'a>: Iterator<Item = &'a [P]>
    where
        P: 'a,
        Self: 'a;

    fn iter_paths(&self) -> Self::ResourceIter<'_>;
}
