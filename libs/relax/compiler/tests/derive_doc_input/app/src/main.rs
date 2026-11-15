// An item's outer doc comments reach its derives and attribute macros as `#[doc = ".."]`
// attributes, as rustc hands them over (the `documented` derive reads a type's `///` lines from
// them). Before: those above the item's first token were dropped ("Missing doc comments").
use derive_doc_mac::{echo_attr, Echo};

/// One line.
///
/// A ` `code` ` and "quotes".
#[derive(Echo)]
pub struct Outer {
    /// a field
    pub x: u8,
}

/** A block comment. */
#[derive(Echo)]
struct Block;

#[doc = "explicit"]
#[derive(Echo)]
struct Explicit;

// a plain comment is not a doc
/// Attr.
#[echo_attr]
struct Attr;

fn main() {
    /// Local.
    #[derive(Echo)]
    struct Local;
    println!("{}", Outer::INPUT);
    println!("{}", Block::INPUT);
    println!("{}", Explicit::INPUT);
    println!("{}", Attr::INPUT);
    println!("{}", Local::INPUT);
}
