use crate::Transformation;

/// A trait to transform geometric objects in 2-dimensional Euclidian space.
pub trait Transform {
    fn transform<T>(self, t: &T) -> Self
    where
        T: Transformation;

    fn transform_mut<T>(&mut self, t: &T)
    where
        T: Transformation;
}
