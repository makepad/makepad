mod logs {
    #[macro_export]
    macro_rules! logit { ($($t:tt)*) => { println!("out logged {}", format!($($t)*)) } }
}
mod prelude { pub use crate::logit; }
mod a {
    pub fn f() { println!("out logged x 1"); }
}
#[test]
fn run() { a::f(); }
