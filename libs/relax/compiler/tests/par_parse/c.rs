pub fn which() -> i32 { m!() }
pub fn own() -> i32 {
    macro_rules! m { () => { 3 } }
    m!()
}
pub fn path() -> &'static str { module_path!() }
