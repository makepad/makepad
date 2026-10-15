//~ msg: cannot find value
// The active module has only a glob of a named-field struct, not a value
// constructor. The other module's function is removed by cfg. rustc: E0423.
#[cfg(any())]
pub mod nested {
    pub fn Item() -> super::Item { super::Item {} }
}
#[cfg(not(any()))]
pub mod nested { use super::*; }
pub struct Item {}
fn accept<T>(_: fn() -> T) {}
fn main() { accept(nested::Item); }
