mod logs {
    #[macro_export]
    macro_rules! logit { ($($t:tt)*) => { println!("out logged {}", format!($($t)*)) } }
}
mod a {
    use crate::logit;
    pub fn f() { logit!("z {}", 3); }
}
#[test]
fn run() { a::f(); }
