mod logs {
    #[macro_export]
    macro_rules! logit { ($($t:tt)*) => { println!("out logged {}", format!($($t)*)) } }
}
fn f() { use crate::logit; logit!("x {}", 1); }
#[test]
fn run() { f(); }
