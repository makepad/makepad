//! A file module cannot export a macro to a preceding call.
fn before() -> u32 { ordered_value!() }
#[macro_use]
#[path = "macro_file_order/definitions.rs"]
mod definitions;
#[test]
fn value() { assert_eq!(before(), 42); }
