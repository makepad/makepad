//! A panic of JIT-compiled code through the provided crate's rustc frames: rustc's drop guard
//! runs and rustc's catch_unwind catches it, as when both sides are rustc's (see Cargo.toml).
use relax_provided_fixture::{catch_through, guard_drops, through_guard};
use std::panic::{catch_unwind, AssertUnwindSafe};

fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    let mut boom = || -> u32 { panic!("the caller's side panicked") };
    let mut fine = || -> u32 { 41 };
    let returned = through_guard(&mut fine);
    let through = catch_unwind(AssertUnwindSafe(|| through_guard(&mut boom))).is_err();
    let caught = catch_through(&mut boom);
    let quiet = catch_through(&mut fine);
    println!("unwind through returned {} caught above {} guard drops {} rustc caught {} quiet {}", returned, through, guard_drops(), caught, quiet);
}
