//! `use apm_dep::apm_dep;` imports dep's exported macro `apm_dep!` but not its private
//! `mod apm_dep`, so the extern crate `apm_dep` still names the crate for `#[apm_dep::generate]`;
//! a public module alias still resolves an attribute. `use apm_dep::service;` is the attribute dep
//! re-exports beside its private `mod service`, here and re-exported again from a module.
//! `cargo run -p apm-app` prints `apm 17 served 18`.
use apm_dep::apm_dep;

#[apm_dep::generate]
struct Original;

mod public_import {
    use apm_dep::public_module as attrs;
    #[attrs::generate]
    pub struct Original;
}

use apm_dep::service;

#[service]
struct Front;

mod again {
    pub use apm_dep::service;
}

mod served_again {
    #[super::again::service]
    pub struct Back;
}

fn main() {
    let _ = (Original, Generated, public_import::Original, public_import::Generated, Front, served_again::Back);
    println!("apm {} served {}", apm_dep!(), SERVED + served_again::SERVED);
}
