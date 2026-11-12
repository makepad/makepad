extern "C" {
    fn cn_answer() -> i32;
}
pub fn answer() -> i32 { unsafe { cn_answer() + 2 } }
pub fn built() -> bool { cfg!(cnative_built) }
