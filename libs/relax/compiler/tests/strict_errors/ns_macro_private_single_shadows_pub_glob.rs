//~ msg: cannot find macro `m` in this scope
pub mod a {
    #[macro_export]
    macro_rules! ma { () => {} }
    pub use crate::ma as m;
}
pub mod b {
    pub use crate::a::m;
    pub fn anchor() {}
}
mod bridge {
    use crate::a::m;
    pub use crate::b::*;
    pub fn keep() { anchor(); m!(); }
}
mod caller {
    use crate::bridge::*;
    pub fn call() { keep(); m!(); }
}
fn main() { caller::call(); }
