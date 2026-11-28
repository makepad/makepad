//~ msg: cannot find macro `script_eval` in this scope
mod a {
    use crate::*;
    pub fn keep() { anchor(); }
    pub mod child {
        use crate::outsider::*;
        pub fn call() { keep(); script_eval!(); }
    }
}
mod outsider { pub use crate::a::*; }
pub fn anchor() {}
#[macro_export]
macro_rules! script_eval { () => {} }
fn main() { a::child::call(); }
