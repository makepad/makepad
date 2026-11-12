//~ none
// The positions where rustc configures an expansion's expression attribute without the
// `stmt_expr_attributes` gate: a statement (its `;` kept), a block's tail, a list element by
// itself (an array's, a tuple's, a call's or a method call's argument, a `vec!` element) and a
// method call's receiver (tests/strict_errors/e0658_expr_attr_in_local_macro.rs has the gated ones).
macro_rules! five { () => { #[cfg(all())] 5u32 } }
fn g(x: u32) -> u32 { x }
fn f() -> u32 {
    five!();
    let a = [five!()][0] + g(five!()) + (five!(), 1).0 + (five!(),).0 + 0u32.max(five!());
    a + five!().count_ones() + vec![five!(); 1][0] + { five!() }
}
fn main() { f(); }
