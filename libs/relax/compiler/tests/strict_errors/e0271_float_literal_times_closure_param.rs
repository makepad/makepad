//~ E0271
//~ help: type mismatch resolving
// An arithmetic operator's left operand is checked with no expected type (rustc op.rs
// check_overloaded_binop), and with an unannotated closure parameter on the right the operator
// is `<{float} as Mul<?S>>` until fallback, not the built-in one: the literal falls back to f64,
// whose product is no f32 even once the call makes `sign` a float.
fn main() {
    let scaled = |sign| {
        let mut v = vec![0.0f32; 1];
        v[0] = 0.75 * sign;
        v
    };
    let _ = scaled(1.0);
}
