// The outer attributes a derive's input carries (relax issue 106): rustc hands a derive the item
// with its doc comments (`#[doc = ".."]`), those before its first attribute included.
use did_mac::Attrs;

/// Doc of a struct before its derive.
#[derive(Attrs)]
struct Plain {
    /// Field doc.
    field: u32,
}

/// Doc of an item that derives nothing; it is not the next item's.
#[allow(dead_code)]
struct NoDerive;

// a plain comment, and //// four slashes: no docs
//// not a doc either
/// First line,
/// second line with "quotes".
#[derive(Attrs)]
/// After the derive.
#[doc = "explicit"]
/** Block doc. */
enum Mixed {
    /// Variant doc.
    A,
}

#[derive(Attrs)]
struct Undocumented(#[allow(dead_code)] u8);

macro_rules! documented {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Attrs)]
        struct $name;
    };
}
documented! {
    /// From a macro.
    FromMacro
}

fn main() {
    let _ = Plain { field: 1 }.field;
    let _ = (Mixed::A, Undocumented(0).0, FromMacro);
    println!("{}", Plain::attrs());
    println!("{}", Mixed::attrs());
    println!("{}", Undocumented::attrs());
    println!("{}", FromMacro::attrs());
}
