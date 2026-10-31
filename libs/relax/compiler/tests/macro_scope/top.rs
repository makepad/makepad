// a re-export chain (as makepad_platform: `pub use x::y::inner;`, a module that glob-imports it,
// a root glob of that module) and an import inside a fn body; `crate::log!` / `log!` resolve
// through it to inner's macro, whatever else exports a `log!` in the program
pub use mid::inner;
pub mod logm {
    pub use crate::inner::*;
}
pub use logm::*;
pub mod sub {
    pub fn qualified() -> &'static str { crate::log!() }
}
pub fn unqualified() -> &'static str { log!() }
pub fn in_body() -> &'static str {
    use inner::local_use;
    local_use!()
}
pub fn via_dollar_crate() -> &'static str { inner::wrapped!() }
