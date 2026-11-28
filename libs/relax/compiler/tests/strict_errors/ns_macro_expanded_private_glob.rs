//~ msg: cannot find macro `script_eval` in this scope
mod bridge {
    macro_rules! gen { () => { use crate::*; }; }
    gen!();
    pub fn keep() { script_eval!(); }
}
mod caller {
    use crate::bridge::*;
    pub fn call() { keep(); script_eval!(); }
}
#[macro_export]
macro_rules! script_eval { () => {} }
fn main() { caller::call(); }
