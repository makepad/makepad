mod logs {
    #[macro_export]
    macro_rules! logit { ($($t:tt)*) => { println!("out logged {}", format!($($t)*)) } }
}
mod a {
    pub fn f() { logit!("x {}", 1); }
    use crate::logit;
}
#[test]
fn run() { a::f(); }
