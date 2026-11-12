//! Markdown editing without a window.
//!
//! The markdown text is the only truth. A view over it keeps an [`Index`]:
//! the document's top-level blocks as line ranges that tile every line, each
//! with what its rows need (list items, code lines), plus the document-wide
//! link and footnote tables a slice parse resolves against. An edit reparses
//! a window of blocks around it and stops at the first untouched block that
//! the new parse also starts ([`Index::apply`]). A block's visible text and
//! the source range of each visible character is its [`Projection`].
//!
//! An edit in a rendered view is a [`plan::Plan`]: source replacements a
//! key makes (typing, Enter, Backspace, Delete, a cut) over the blocks around
//! it, with markers kept balanced and containers continued.
//!
//! Nothing here holds a copy of the document: every call takes the
//! document's lines as they are now.

mod cut;
mod format;
mod index;
pub mod plan;
mod project;
mod table;

pub use index::{
    front_matter_end, parse_options, Block, BlockKind, Index, LineEdit, Row, RowPart, Slice,
    Splice, CODE_ROW_LINES,
};
pub use project::{Affinity, Atom, Inline, InlineKind, Piece, PieceKind, Projection};
pub use pulldown_cmark;

#[cfg(test)]
mod tests;
