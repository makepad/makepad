pub const VERSION: &str = "v2";
pub fn value() -> u32 { 2 }
#[macro_export]
macro_rules! which { () => { $crate::value() * 10 } }
pub fn mpath() -> &'static str { module_path!() }
pub fn cname() -> &'static str { env!("CARGO_CRATE_NAME") }
pub struct Thing(pub u32);
pub fn only_v2() -> u32 { 22 }
