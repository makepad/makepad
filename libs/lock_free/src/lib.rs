//! Lock-free plumbing between threads.
//!
//! - [`ring`]: a bounded single-producer single-consumer ring with owned endpoints.
//! - [`SeqCell`]: a seqlock over a `Copy` snapshot with one writer.
//! - [`triple_buffer`]: one writer publishes values and one reader sees the newest.
//! - [`UiCell`]: state owned by exactly one thread behind a shared handle.
//! - [`Slot`]: a value one thread replaces while another uses it in place.
//!
//! Nothing in this crate locks, waits or allocates after construction.

pub mod spsc;
pub mod seq;
pub mod triple;
pub mod ui_cell;
pub mod slot;

pub use seq::SeqCell;
pub use spsc::{ring, Consumer, Producer};
pub use triple::{triple_buffer, Reader, Writer};
pub use ui_cell::UiCell;
pub use slot::Slot;
