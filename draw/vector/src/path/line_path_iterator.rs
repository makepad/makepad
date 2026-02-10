use crate::internal_iter::InternalIterator;
use crate::path::LinePathCommand;

/// An extension trait for iterators over line path commands.
pub trait LinePathIterator: InternalIterator<Item = LinePathCommand> {}

impl<I> LinePathIterator for I where I: InternalIterator<Item = LinePathCommand> {}
