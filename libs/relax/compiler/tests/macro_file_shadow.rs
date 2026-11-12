//! File exports participate in the same textual order as local definitions.
macro_rules! ordered_value { () => { 7u32 }; }
macro_rules! indirect { () => { ordered_value!() }; }
fn before() -> u32 { ordered_value!() }
fn before_expansion() -> u32 { indirect!() }
#[macro_use]
#[path = "macro_file_shadow/definitions.rs"]
mod definitions;
fn middle() -> u32 { ordered_value!() }
fn middle_expansion() -> u32 { indirect!() }
mod child {
    pub fn inherited() -> u32 { ordered_value!() }
    macro_rules! ordered_value { () => { 17u32 }; }
    pub fn local() -> u32 { ordered_value!() }
    pub fn expanded() -> u32 { indirect!() }
}
macro_rules! ordered_value { () => { 99u32 }; }
fn after() -> u32 { ordered_value!() }
fn after_expansion() -> u32 { indirect!() }
#[macro_use]
#[path = "macro_file_shadow/outer.rs"]
mod outer;
#[test]
fn file_exports_and_locals_follow_textual_order() {
    assert_eq!(before(), 7);
    assert_eq!(before_expansion(), 7);
    assert_eq!(middle(), 42);
    assert_eq!(middle_expansion(), 42);
    assert_eq!(child::inherited(), 42);
    assert_eq!(child::local(), 17);
    assert_eq!(child::expanded(), 17);
    assert_eq!(after(), 99);
    assert_eq!(after_expansion(), 99);
    assert_eq!(nested_value!(), 2);
    assert_eq!(outer::before(), 1);
}
