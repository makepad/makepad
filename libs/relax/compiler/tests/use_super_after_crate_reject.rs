// rejects: crate::super
// relax #90: `super` only follows `self` and other `super`s (rustc_resolve resolve_path's
// `allow_super`): `use crate::super as m;` does not resolve, nor does `crate::a::super`.
mod a {
    pub use crate::super as m;
}

#[test]
fn unused() {}
