//~ msg: cannot find macro `my_str` in this scope
mod bridge {
    pub(self) use std::stringify as my_str;
    pub fn keep() -> &'static str { my_str!(bridge) }
}
mod caller {
    use crate::bridge::*;
    pub fn call() -> &'static str { let _ = keep(); my_str!(caller) }
}
fn main() { caller::call(); }
