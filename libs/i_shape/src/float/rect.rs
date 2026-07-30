use crate::base::data::{Path, Shape};
use alloc::vec::Vec;
use i_float::float::compatible::FloatPointCompatible;
use i_float::float::rect::FloatRect;

pub trait RectInit<P>
where
    P: FloatPointCompatible,
{
    fn with_path(path: &[P]) -> Option<FloatRect<P::Scalar>>;
    fn with_paths(paths: &[Vec<P>]) -> Option<FloatRect<P::Scalar>>;
    fn with_list_of_paths(list: &[Vec<Vec<P>>]) -> Option<FloatRect<P::Scalar>>;
}

trait FirstPoint<P>
where
    P: FloatPointCompatible,
{
    fn first_point(&self) -> Option<P>;
}

impl<P> RectInit<P> for FloatRect<P::Scalar>
where
    P: FloatPointCompatible,
{
    fn with_path(path: &[P]) -> Option<FloatRect<P::Scalar>> {
        let &first_point = path.first()?;

        let mut rect = Self::with_point(first_point);

        for p in path.iter() {
            rect.unsafe_add_point(p);
        }

        Some(rect)
    }

    fn with_paths(paths: &[Path<P>]) -> Option<FloatRect<P::Scalar>> {
        let first_point = paths.first_point()?;

        let mut rect = Self::with_point(first_point);

        for path in paths.iter() {
            for p in path.iter() {
                rect.unsafe_add_point(p);
            }
        }

        Some(rect)
    }

    fn with_list_of_paths(list: &[Vec<Path<P>>]) -> Option<FloatRect<P::Scalar>> {
        let first_point = list.first_point()?;

        let mut rect = Self::with_point(first_point);

        for paths in list.iter() {
            for path in paths.iter() {
                for p in path.iter() {
                    rect.unsafe_add_point(p);
                }
            }
        }

        Some(rect)
    }
}

impl<P> FirstPoint<P> for [Path<P>]
where
    P: FloatPointCompatible,
{
    fn first_point(&self) -> Option<P> {
        for path in self.iter() {
            if let Some(&p) = path.first() {
                return Some(p);
            }
        }
        None
    }
}

impl<P> FirstPoint<P> for [Shape<P>]
where
    P: FloatPointCompatible,
{
    fn first_point(&self) -> Option<P> {
        for paths in self.iter() {
            for path in paths.iter() {
                if let Some(&p) = path.first() {
                    return Some(p);
                }
            }
        }
        None
    }
}
