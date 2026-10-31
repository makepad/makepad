//~ none
// the glob variant: the fn comes through `pub use validate::*;`, the module `validate` stays a module
mod validate {
    use super::*;
    pub fn validate() {}
}
pub use validate::*;
fn main() { validate(); validate::validate(); }
