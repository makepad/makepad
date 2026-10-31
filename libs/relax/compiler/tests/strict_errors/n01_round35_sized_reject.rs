//~ E0277
// rustc_hir_typeck/src/expr.rs:660-703 @8d1a764: unused fn-item paths too.
extern "C" {
    fn input(value: str);
    fn output() -> str;
}
fn main() {
    let _ = input;
    let _ = output;
}
