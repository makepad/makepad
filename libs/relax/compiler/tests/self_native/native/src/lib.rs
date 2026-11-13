extern "C" {
    fn sn_value() -> i32;
}
pub fn value() -> i32 {
    unsafe { sn_value() }
}
