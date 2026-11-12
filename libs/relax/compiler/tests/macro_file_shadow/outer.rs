#[macro_use]
#[path = "outer/inner.rs"]
mod inner;
pub fn before() -> u32 { nested_value!() }
macro_rules! nested_value { () => { 2u32 }; }
