pub const VERSION: &str = "v1";
pub fn value() -> u32 { 1 }
#[macro_export]
macro_rules! which { () => { $crate::value() } }
pub fn mpath() -> &'static str { module_path!() }
pub fn cname() -> &'static str { env!("CARGO_CRATE_NAME") }
pub struct Thing(pub u32);
