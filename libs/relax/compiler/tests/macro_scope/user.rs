// imports dep_b's `trace` by `use`: an unqualified `trace!` is that one, whatever else the
// program loaded before this crate (rustc_resolve: never another crate's export it did not import)
use dep_b::log::trace;
pub fn which() -> &'static str { trace!("x") }
