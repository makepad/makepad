extern "C" {
    fn cn_add(a: i32, b: i32) -> i32;
    fn cn_hash(p: *const u8, n: usize) -> u64;
    fn cn_apply(f: extern "C" fn(i32) -> i32, x: i32) -> i32;
    fn cn_square_plus(x: i32) -> i32;
}
pub fn add(a: i32, b: i32) -> i32 { unsafe { cn_add(a, b) } }
pub fn hash(b: &[u8]) -> u64 { unsafe { cn_hash(b.as_ptr(), b.len()) } }
pub fn apply(f: extern "C" fn(i32) -> i32, x: i32) -> i32 { unsafe { cn_apply(f, x) } }
pub fn built() -> bool { cfg!(cnative_built) }
/// Calls the C code, which calls back into Rust by symbol name.
pub fn square_plus(x: i32) -> i32 { unsafe { cn_square_plus(x) } }
/// Only C calls it.
#[no_mangle]
pub extern "C" fn cn_rust_square(x: i32) -> i32 { x * x }
