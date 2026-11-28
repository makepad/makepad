//~ msg: cannot find macro `script_eval` in this scope
mod a {
    pub mod bridge {
        pub(super) use crate::*;
        pub fn keep() { anchor(); }
    }
}
mod caller {
    use crate::a::bridge::*;
    pub fn call() { keep(); script_eval!(); }
}
pub fn anchor() {}
#[macro_export]
macro_rules! script_eval { () => {} }
fn main() { caller::call(); }
