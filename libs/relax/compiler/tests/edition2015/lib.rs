// boot b17 (frontend, imports): in an edition-2015 crate `use name;` / `use name::{self, X};` /
// `use {A, B};` are crate-root relative. objc-sys (edition 2015): `use runtime::{self, Class, ..}`,
// `use {Encode, EncodeArguments, ..}`, `use encode;` -> 14 unresolved imports.
// Booted by run.sh (edition-2015 lib + 2021 bin); rustc --edition 2015: ok.
pub mod runtime {
    pub struct Class;
    pub fn k() -> u32 {
        4
    }
}
pub mod encode {
    pub trait Encode {}
}
pub use encode::Encode;
mod user {
    use runtime::{self, Class};
    use {Encode};
    use encode;
    pub fn f() -> u32 {
        let _c = Class;
        runtime::k()
    }
}
// expr/scope.rs, binding/pat-ranges.rs: `::f()` and `::NUM` (values and range-pattern ends) are
// crate-root paths in 2015
pub const NUM: u32 = 3;
fn root_f() -> u32 {
    1
}
mod inner {
    pub fn g(x: u32) -> u32 {
        match x {
            ::NUM...10 => ::root_f() + ::NUM,
            _ => 0,
        }
    }
}
pub fn f() -> u32 {
    user::f() + inner::g(5) - 4
}
// the injected `extern crate std;` is a crate-root item in 2015 (`::std::..`, `super::std`), and
// a root `extern crate x as y;` puts `y` in the extern prelude for every module (brotli:
// `extern crate alloc_no_stdlib as alloc;` then `alloc::Allocator<u32>` in submodules)
extern crate core as kore;
pub mod prelude_paths {
    use super::std as s;
    pub fn sizes() -> usize {
        kore::mem::size_of::<u64>() + ::std::mem::size_of::<u16>() + s::mem::size_of::<u8>() + ::kore::mem::size_of::<u32>()
    }
}
