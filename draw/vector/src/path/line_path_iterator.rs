use crate::path::LinePathCommand;
use crate::internal_iter::InternalIterator;

/// An extension trait for iterators over line path commands.
pub trait LinePathIterator: InternalIterator<Item = LinePathCommand> {}

impl<I> LinePathIterator for I where I: InternalIterator<Item = LinePathCommand> {}
