//! The Splash designer's engine: structural edits to `script_mod!` source.
//!
//! The tweaker edits a running widget's VALUES by evaluating Splash chunks
//! against it; that never writes source and cannot add, remove or move
//! children. The designer edits the SOURCE TEXT of a `script_mod!` body and
//! previews the result through hot reload, which rebuilds the widget tree
//! from the edited text without a Rust rebuild. The file is the design: every
//! operation is a byte-range hunk inside one block, and the bytes around it
//! are copied verbatim.
//!
//! The parts:
//!
//! * [`locate`]: from a live widget to the byte span of the literal that
//!   declares it, through the runtime's own source map, checked against a
//!   fresh scan of the file so a stale file fails closed.
//! * [`doc`]: the working text of one file, its base, the hunks made so far
//!   and the undo stack, plus the gate that refuses an edit hot reload would
//!   refuse.
//! * [`ops`]: the operations (insert, delete, move, duplicate, wrap, rename,
//!   set a property) as pure text edits over a located node.
//!
//! The app never writes the file: a commit is a patch the caller applies.

pub mod doc;
pub mod locate;
pub mod ops;
pub mod text;

pub use doc::{DesignDoc, Hunk, PreviewOutcome};
pub use locate::{locate_widget, NodeSpan};
pub use ops::{DesignOp, Placement};
