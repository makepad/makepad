use crate::base::data::{Contour, Shape};
use i_float::float::compatible::FloatPointCompatible;

pub trait PointsCount<P> {
    fn points_count(&self) -> usize;
}

impl<P> PointsCount<P> for [Contour<P>]
where
    P: FloatPointCompatible,
{
    fn points_count(&self) -> usize {
        self.iter().fold(0, |acc, list| acc + list.len())
    }
}

impl<P> PointsCount<P> for [Shape<P>]
where
    P: FloatPointCompatible,
{
    fn points_count(&self) -> usize {
        self.iter().fold(0, |acc, lists| acc + lists.points_count())
    }
}
