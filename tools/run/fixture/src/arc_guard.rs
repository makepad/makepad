//! See Cargo.toml: an unmarked fn of the provided crate counting rustc's Arc through its address.
fn main() {
    let at = relax_provided_fixture::shared_registry();
    println!("strong {}", relax_provided_fixture::shared_registry_retain(at));
}
