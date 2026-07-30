use crate::core::overlay::ShapeType;

pub(crate) trait WindingCount
where
    Self: Clone + Copy + Send + Sync,
{
    fn is_not_empty(&self) -> bool;
    fn new(subj: i32, clip: i32) -> Self;
    fn with_shape_type(shape_type: ShapeType) -> (Self, Self);
    fn direct_count(shape_type: ShapeType) -> Self;
    fn invert_count(shape_type: ShapeType) -> Self;
    fn add(self, count: Self) -> Self;
    fn invert(self) -> Self;
}
