// Calls one member of a static library whose other members call symbols nothing defines.
extern "C" {
    fn acu_answer() -> i32;
    #[cfg(feature = "needed")]
    fn acu_needed_api() -> i32;
}

fn main() {
    println!("answer {}", unsafe { acu_answer() });
    #[cfg(feature = "needed")]
    println!("needed {}", unsafe { acu_needed_api() });
}
