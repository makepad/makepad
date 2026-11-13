extern "C" {
    fn snd_twice(x: i32) -> i32;
}
pub fn twice(x: i32) -> i32 {
    unsafe { snd_twice(x) }
}
