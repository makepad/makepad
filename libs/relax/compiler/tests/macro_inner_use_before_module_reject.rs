//! Inner macro_use does not export a macro to an earlier sibling module.
mod before {
    pub fn value() -> u32 { later_value!() }
}
mod definitions {
    #![macro_use]
    macro_rules! later_value { () => { 17u32 }; }
}
fn main() { let _ = before::value(); }
