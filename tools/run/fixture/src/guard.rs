//! See Cargo.toml: a generic fn of the provided crate reading an opaque field.
fn main() {
    let mut r = relax_provided_fixture::Registry::new();
    r.add("x");
    println!("{}", relax_provided_fixture::names_peek(&r, 0u8));
}
