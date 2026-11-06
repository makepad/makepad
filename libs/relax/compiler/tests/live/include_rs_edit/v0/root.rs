// A live save of a Rust file the code include!s patches the code it expands into: the file is no
// source of the crate (its text is the includer's expansion), so the includer takes the edit. v1
// saves items.rs alone (an item-level include!), v2 value.rs alone (an include! in a fn body).
include!("items.rs");

fn value() -> u32 {
    include!("value.rs")
}

#[test]
fn run() {
    println!("out item {}", item());
    println!("out value {}", value());
}
