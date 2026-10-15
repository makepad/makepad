// `U::from(x)` in a generic body with U: From<T> calls From::from, even when the instance's U
// has an inherent `from` with other arguments (rustc resolves `U::item` through U's bounds on
// the generic body). uizoo at fac47173c7: `.into()` to a type with an inherent
// `fn from(&mut self, cx, t, props, o)` (makepad TweenHost) -> "expected 5 arguments, found 1".
struct Host(u32);
impl Host {
    #[allow(dead_code)]
    fn from(&mut self, a: u32, b: u32) -> u32 { self.0 + a + b }
}
impl From<u32> for Host {
    fn from(x: u32) -> Host { Host(x * 2) }
}
fn conv<T, U: From<T>>(x: T) -> U { U::from(x) }
trait Make { fn make() -> Self; }
struct M(u8);
impl M { #[allow(dead_code)] fn make(_x: u8) -> M { M(1) } }
impl Make for M { fn make() -> M { M(7) } }
fn mk<T: Make>() -> T { T::make() }
#[test] fn a_into_uses_from_trait() { let h: Host = 21u32.into(); assert_eq!(h.0, 42); }
#[test] fn b_param_path_bound() { let h: Host = conv(5u32); assert_eq!(h.0, 10); }
#[test] fn c_static_trait_fn() { let m: M = mk(); assert_eq!(m.0, 7); }
// `self.draw(..)` in a trait's default method is the trait's `draw` (Self is a param there),
// even when the implementing type has an inherent `draw` with other parameters. uizoo:
// Widget::draw_all<Tab> -> "mismatched types: expected `&str`, found `&mut Scope`".
trait Draw {
    fn draw(&mut self, n: u32) -> u32;
    fn draw_all(&mut self) -> u32 { self.draw(1) + self.draw(2) }
}
struct Tab(u32);
impl Tab {
    #[allow(dead_code)]
    fn draw(&mut self, name: &str) -> u32 { name.len() as u32 }
}
impl Draw for Tab {
    fn draw(&mut self, n: u32) -> u32 { self.0 += n; self.0 }
}
#[test] fn d_default_method_self_call() { let mut t = Tab(10); assert_eq!(t.draw_all(), 11 + 13); }
