//~ none
//! The ways a module names a macro that rustc accepts: textual scope after a `macro_rules!` (into
//! the modules declared after it), `#[macro_use]` on the module that defines it, an import of
//! the crate root's re-export (`use crate::name;`, `crate::name!`), and a direct child's glob of
//! the root (its imports and `#[macro_export]` macros, defined before or after).
macro_rules! early { ($e:expr) => { $e } }
#[macro_use]
mod defs {
    macro_rules! used { ($e:expr) => { $e } }
    #[macro_export]
    macro_rules! inner_eval { ($e:expr) => { $e } }
}
pub use crate::inner_eval as script_eval;
pub mod widget_async {
    pub fn f() -> u32 { early!(1) + used!(2) }
    pub mod tests {
        use crate::script_eval;
        pub fn family() -> u32 { script_eval!(1) + crate::script_eval!(2) + super::f() }
    }
}
pub mod child {
    use super::*;
    pub fn g() -> u32 { script_eval!(3) + later!(4) }
}
#[macro_export]
macro_rules! later { ($e:expr) => { $e } }
fn main() { let _ = (script_eval!(5), child::g(), widget_async::tests::family()); }
