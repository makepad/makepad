//~ E0433
pub mod target { pub use crate::script_eval; }
mod bridge {
    use crate::*;
    pub fn keep() { anchor(); }
}
mod caller {
    use crate::bridge::*;
    pub fn call() { keep(); target::script_eval!(); }
}
pub fn anchor() {}
#[macro_export]
macro_rules! script_eval { () => {} }
fn main() { caller::call(); }
