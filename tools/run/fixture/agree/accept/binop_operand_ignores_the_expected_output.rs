// `f64 * Vec3 -> Vec3`: what the context expects of the product says nothing about the operands.
use core::ops::Mul;
#[derive(Clone, Copy)]
pub struct Vec3(pub f64);
impl Mul<Vec3> for f64 { type Output = Vec3; fn mul(self, v: Vec3) -> Vec3 { Vec3(self * v.0) } }
fn take(v: Vec3) -> f64 { v.0 }
pub fn pick(len: f64, flag: bool, a: Vec3, b: Vec3) -> f64 {
    take(len * if flag { a } else { b })
}
#[test] fn runs() { assert_eq!(pick(2.0, true, Vec3(3.0), Vec3(4.0)), 6.0); }
