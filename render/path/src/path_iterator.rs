use crate::{LinePathCommand, PathCommand};
use geometry::QuadraticSegment;
use internal_iter::InternalIterator;

/// An extension trait for iterators over path commands.
pub trait PathIterator: InternalIterator<Item = PathCommand> {
    /// Returns an iterator over line path commands that approximate `self` with tolerance
    /// `epsilon`.
    fn linearize(self, epsilon: f32) -> Linearize<Self>
    where
        Self: Sized,
    {
        Linearize {
            path: self,
            epsilon,
        }
    }
}

impl<I> PathIterator for I where I: InternalIterator<Item = PathCommand> {}

/// An iterator over line path commands that approximate `self` with tolerance `epsilon`.
#[derive(Clone, Debug)]
pub struct Linearize<P> {
    path: P,
    epsilon: f32,
}

impl<P> InternalIterator for Linearize<P>
where
    P: PathIterator,
{
    type Item = LinePathCommand;

    fn for_each<F>(self, f: &mut F) -> bool
    where
        F: FnMut(LinePathCommand) -> bool,
    {
        let mut initial_point = None;
        let mut current_point = None;
        self.path.for_each({
            let epsilon = self.epsilon;
            &mut move |command| match command {
                PathCommand::MoveTo(p) => {
                    initial_point = Some(p);
                    current_point = Some(p);
                    f(LinePathCommand::MoveTo(p))
                }
                PathCommand::LineTo(p) => {
                    current_point = Some(p);
                    f(LinePathCommand::LineTo(p))
                }
                PathCommand::QuadraticTo(p1, p) => {
                    QuadraticSegment::new(current_point.unwrap(), p1, p)
                        .linearize(epsilon)
                        .for_each(&mut |p| {
                            current_point = Some(p);
                            f(LinePathCommand::LineTo(p))
                        })
                }
                PathCommand::Close => {
                    current_point = initial_point;
                    f(LinePathCommand::Close)
                }
            }
        })
    }
}
