use crate::geometry::{Point, Transform, Transformation};

/// A command in a path
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathCommand {
    MoveTo(Point),
    LineTo(Point),
    ArcTo(Point, Point, f64, bool, bool),
    QuadraticTo(Point, Point),
    CubicTo(Point, Point, Point),
    Close,
}

impl Transform for PathCommand {
    fn transform<T>(self, t: &T) -> PathCommand
    where
        T: Transformation,
    {
        match self {
            PathCommand::ArcTo(e, r, xr, l, s) => {
                PathCommand::ArcTo(e.transform(t), r.transform(t), xr, l, s)
            }
            PathCommand::MoveTo(p) => PathCommand::MoveTo(p.transform(t)),
            PathCommand::LineTo(p) => PathCommand::LineTo(p.transform(t)),
            PathCommand::QuadraticTo(p1, p) => {
                PathCommand::QuadraticTo(p1.transform(t), p.transform(t))
            }
            PathCommand::CubicTo(p1, p2, p) => {
                PathCommand::CubicTo(p1.transform(t), p2.transform(t), p.transform(t))
            }
            PathCommand::Close => PathCommand::Close,
        }
    }

    fn transform_mut<T>(&mut self, t: &T)
    where
        T: Transformation,
    {
        *self = self.transform(t);
    }
}

