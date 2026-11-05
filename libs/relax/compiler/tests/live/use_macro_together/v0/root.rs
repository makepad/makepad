mod logs {
    #[macro_export]
    macro_rules! logit { ($($t:tt)*) => { println!("out logged {}", format!($($t)*)) } }
}
mod a;
mod b;
#[test]
fn run() { b::f(); a::g(); }
