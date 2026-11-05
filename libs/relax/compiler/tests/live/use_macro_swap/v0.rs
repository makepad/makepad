mod one { #[macro_export] macro_rules! pick_one { () => { "one" } } }
mod two { #[macro_export] macro_rules! pick_two { () => { "two" } } }
mod a {
    use crate::pick_one as pick;
    pub fn f() -> &'static str { pick!() }
}
#[test]
fn run() { println!("out {}", a::f()); }
