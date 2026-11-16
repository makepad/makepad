// rejects: no implementation for `u8 & i32`
// A shift whose right operand is an unannotated closure parameter is an operator trait call (rustc
// op.rs check_expr_binop: only operands that are not type variables are built in). Its type stays
// `<u8 as Shr<?N>>::Output` until integer fallback makes `n` and `1` i32: `u8: BitAnd<i32>` is unmet.
pub fn f(a: u8) -> bool {
    let g = |n| (a >> n) & 1 != 0;
    g(3)
}
