// rejects: E0365: `super` is only public within the crate, and cannot be re-exported outside
// relax #90: `pub use super::super as m;` re-exports the module `a`, which is private to the crate
// root, so rustc_resolve's finalize_import reports E0365 for it, naming the keyword.
mod a {
    pub mod b {
        pub mod c {
            pub use super::super as a_mod;
        }
    }
}

#[test]
fn unused() {}
