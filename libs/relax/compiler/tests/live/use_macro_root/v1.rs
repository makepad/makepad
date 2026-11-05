mod logs {
    #[macro_export]
    macro_rules! logit { ($($t:tt)*) => { println!("out logged {}", format!($($t)*)) } }
}
mod a {
    use crate::logit;
    pub fn f() { logit!("x {}", 1); }
}
#[test]
fn run() { a::f(); }
