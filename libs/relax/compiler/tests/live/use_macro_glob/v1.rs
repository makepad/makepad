mod logs {
    #[macro_export]
    macro_rules! logit { ($($t:tt)*) => { println!("out logged {}", format!($($t)*)) } }
}
mod prelude { pub use crate::logit; }
mod a {
    use crate::prelude::*;
    pub fn f() { logit!("x {}", 1); }
}
#[test]
fn run() { a::f(); }
