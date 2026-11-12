//! A file module's macros become visible after its declaration.
macro_rules! ordered_value { () => { 7u32 }; }
fn before() -> u32 { ordered_value!() }
macro_rules! indirect { () => { ordered_value!() }; }
fn before_expansion() -> u32 { indirect!() }
#[macro_use]
#[path = "macro_file_order/definitions.rs"]
mod definitions;
fn after() -> u32 { new_value!() }
macro_rules! after_indirect { () => { new_value!() }; }
fn after_expansion() -> u32 { after_indirect!() }
#[test]
fn file_module_macro_order() {
    assert_eq!(before(), 7);
    assert_eq!(before_expansion(), 7);
    assert_eq!(after(), 42);
    assert_eq!(after_expansion(), 42);
}
