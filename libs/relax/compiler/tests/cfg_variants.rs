// Enum variants whose #[cfg] is false are stripped before anything sees them (rustc): their
// types are never resolved, they take no layout space, derives skip them. And an operator with
// several Rhs impls and an open literal operand selects the one impl the literal fits
// (`v * 0.5` with Mul<V> and Mul<f32>: f32, never the f64 fallback). Lines "| " vs rustc.
use std::ops::{Div, Mul};

#[derive(Debug, Clone, PartialEq)]
enum Gen {
    Stub(u32),
    #[cfg(feature = "video")]
    H3(h3_gen::H3Gen),
    #[cfg(target_os = "windows")]
    Win { body: WinHttpBody },
    #[cfg(not(target_os = "windows"))]
    Unix { fd: i32 },
    Last,
}

#[derive(Clone, Copy, Debug)]
struct V(f32);
impl Mul<V> for V { type Output = V; fn mul(self, o: V) -> V { V(self.0 * o.0) } }
impl Mul<f32> for V { type Output = V; fn mul(self, o: f32) -> V { V(self.0 * o) } }
impl Div<V> for V { type Output = V; fn div(self, o: V) -> V { V(self.0 / o.0) } }
impl Div<f32> for V { type Output = V; fn div(self, o: f32) -> V { V(self.0 / o) } }

#[test]
fn v01_cfg_variants() {
    let g = Gen::Stub(3);
    if let Gen::Stub(x) = g.clone() {
        println!("| {}", x);
    }
    println!("| {} {:?} {:?} {}", std::mem::size_of::<Gen>(), Gen::Unix { fd: 4 }, Gen::Last, g == Gen::Stub(3));
}

#[test]
fn v02_literal_rhs() {
    let v = V(3.0);
    let k = 4.0;
    println!("| {:?} {:?} {:?} {:?}", v * 0.5, v / 2.0, v * k, (v * 2.0) / (1.0 + 1.0));
}
