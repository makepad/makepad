//! `use apm_dep::apm_dep;` imports dep's exported macro `apm_dep!` but not its private
//! `mod apm_dep`, so the extern crate `apm_dep` still names the crate for `#[apm_dep::generate]`;
//! a public module alias still resolves an attribute. `cargo run -p apm-app` prints `apm 17`.
use apm_dep::apm_dep;

#[apm_dep::generate]
struct Original;

mod public_import {
    use apm_dep::public_module as attrs;
    #[attrs::generate]
    pub struct Original;
}

fn main() {
    let _ = (Original, Generated, public_import::Original, public_import::Generated);
    println!("apm {}", apm_dep!());
}
