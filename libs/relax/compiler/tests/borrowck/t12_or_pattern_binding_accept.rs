// Borrowck regression found by the N01 Gandalf audit (round 52); run_live_regressions.sh checks it.
// Regression (rustc accepts; Relax once rejected it): a binding bound in every alternative of nested or-patterns
// `(A(v) | B(v), _) | (_, A(v) | B(v))` is reported as E0381 "used binding `v` isn't
// initialized" (makepad rx_core value_dump.rs:430).
pub enum Scalar { Number(f64), Vector(Vec<f64>), Color(Vec<f64>) }
pub fn width(left: &Scalar, right: &Scalar) -> usize {
    match (left, right) {
        (Scalar::Vector(v) | Scalar::Color(v), _) | (_, Scalar::Vector(v) | Scalar::Color(v)) => v.len(),
        _ => 0,
    }
}
fn run() {
    assert_eq!(width(&Scalar::Number(1.0), &Scalar::Color(vec![1.0, 2.0])), 2);
    assert_eq!(width(&Scalar::Vector(vec![1.0]), &Scalar::Number(1.0)), 1);
}
#[test]
fn or_pattern_binding() { run() }
fn main() { run() }
