//~ none
// A module and a fn of one name: `use m::m;` re-exports the fn only (the glob-imported module `m`
// inside `m` is private to it), clashes are per namespace.
mod validate {
    use super::*;
    pub fn validate() {}
}
pub use validate::validate;
fn main() { validate(); }
