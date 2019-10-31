use crate::LinePathCommand;
use internal_iter::InternalIterator;

/// An extension trait for iterators over line path commands.
pub trait LinePathIterator: InternalIterator<Item = LinePathCommand> {
    /// Returns an iterator over line path commands that offsets each point with the given
    /// `distance` in the direction of the normal of that point.
    fn dilate(self, distance: f32) -> Dilate<Self>
    where
        Self: Sized,
    {
        Dilate {
            path: self,
            distance,
        }
    }
}

impl<I> LinePathIterator for I where I: InternalIterator<Item = LinePathCommand> {}

/// An iterator over line path commands that offsets each point with the given `distance` in the
/// direction of the normal of that point.
///
/// The normal of a point is defined as the average of the normals of the two line segments incident
/// to the point.
#[derive(Clone, Debug)]
pub struct Dilate<P> {
    path: P,
    distance: f32,
}

impl<P> InternalIterator for Dilate<P>
where
    P: LinePathIterator,
{
    type Item = LinePathCommand;

    fn for_each<F>(self, f: &mut F) -> bool
    where
        F: FnMut(LinePathCommand) -> bool,
    {
        let mut points = Vec::new();
        self.path.for_each({
            let distance = self.distance;
            &mut move |command| match command {
            LinePathCommand::MoveTo(p) => {
                // TODO: Add support for dilating open paths
                assert!(points.is_empty());
                points.push(p);
                true
            }
            LinePathCommand::LineTo(p) => {
                points.push(p);
                true
            }
            LinePathCommand::Close => {
                points.dedup();
                if points[points.len() - 1] == points[0] {
                    points.pop();
                }
                for index in 0..points.len() {
                    let previous_point = if index == 0 {
                        points[points.len() - 1]
                    } else {
                        points[index - 1]
                    };
                    let point = points[index];
                    let next_point = if index == points.len() - 1 {
                        points[0]
                    } else {
                        points[index + 1]
                    };
                    let previous_normal = (point - previous_point)
                        .perpendicular()
                        .normalize()
                        .unwrap();
                    let next_normal = (next_point - point).perpendicular().normalize().unwrap();
                    let normal = (previous_normal + next_normal).normalize().unwrap();
                    let offset = point + normal * distance;
                    if index == 0 {
                        if !f(LinePathCommand::MoveTo(offset)) {
                            return false;
                        }
                    } else {
                        if !f(LinePathCommand::LineTo(offset)) {
                            return false;
                        }
                    }
                }
                if !f(LinePathCommand::Close) {
                    return false;
                }
                points.clear();
                true
            }
        }})
    }
}
