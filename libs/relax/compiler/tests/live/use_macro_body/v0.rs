mod logs {
    #[macro_export]
    macro_rules! logit { ($($t:tt)*) => { println!("out logged {}", format!($($t)*)) } }
}
fn f() { println!("out logged x 1"); }
#[test]
fn run() { f(); }
