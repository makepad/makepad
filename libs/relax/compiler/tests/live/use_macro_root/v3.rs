mod logs {
    #[macro_export]
    macro_rules! logit { ($($t:tt)*) => { println!("out logged {}", format!($($t)*)) } }
}
mod a {
    pub fn f() { logit!("y {}", 2); }
}
#[test]
fn run() { a::f(); }
