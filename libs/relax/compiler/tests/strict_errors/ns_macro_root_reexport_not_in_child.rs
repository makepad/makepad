//~ msg: cannot find macro `script_eval` in this scope
//! A macro the crate root re-exports is in the root's scope only: a module two levels down whose
//! glob import (`use super::*`) reaches a parent that does not import it cannot call it by its
//! plain name (rustc: a plain name's module scope is the current module's own imports, not its
//! parents'). `crate::script_eval!` or `use crate::script_eval;` is required.
mod defs {
    #[macro_export]
    macro_rules! inner_eval { ($e:expr) => { $e } }
}
pub use crate::inner_eval as script_eval;
mod draw { pub fn helper() -> u32 { 0 } }
pub mod widget_async {
    use crate::draw::*;
    pub fn f() -> u32 { helper() }
    pub mod tests {
        use super::*;
        pub fn family() -> u32 { script_eval!(1) + f() }
    }
}
fn main() { let _ = widget_async::tests::family(); }
