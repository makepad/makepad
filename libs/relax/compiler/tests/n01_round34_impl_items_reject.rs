trait Tr { type T; const N: usize; fn f(x: i32) -> i32; }
struct S;
impl Tr for S {}
trait Method { fn safe(); fn arg(x: u32); fn ret() -> u32; }
impl Method for S { unsafe fn safe() {} fn arg(x: i32) {} fn ret() -> i32 { 0 } }
