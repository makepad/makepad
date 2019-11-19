use geometry::{Point, Transform, Transformation};

/// A command in a line path
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LinePathCommand {
    MoveTo(Point),
    LineTo(Point),
    Close,
}

impl Transform for LinePathCommand {
    fn transform<T>(self, t: &T) -> LinePathCommand
    where
        T: Transformation,
    {
        match self {
            LinePathCommand::MoveTo(p) => LinePathCommand::MoveTo(p.transform(t)),
            LinePathCommand::LineTo(p) => LinePathCommand::LineTo(p.transform(t)),
            LinePathCommand::Close => LinePathCommand::Close,
        }
    }

    fn transform_mut<T>(&mut self, t: &T)
    where
        T: Transformation,
    {
        *self = self.transform(t);
    }
}
