mod next;

pub use self::next::Next;

use std::{
    pin::Pin,
    task::{Context, Poll},
};

pub trait Stream {
    type Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>>;

    fn next(&mut self) -> Next<'_, Self>
    where
        Self: Unpin,
    {
        Next::new(self)
    }
}
