//! A call checks its closure arguments after the others (rustc fn_ctxt/checks.rs
//! check_argument_types), and a closure in parentheses is one: HIR has none. peg's generated
//! parsers call `call_custom_closure((|input, pos| input.ident(pos)), __input, __pos)`, whose
//! closure learns `input` from `__input`.
fn call_custom<I: ?Sized, T>(f: impl FnOnce(&I, usize) -> T, input: &I, pos: usize) -> T {
    f(input, pos)
}

struct Input;

impl Input {
    fn ident(&self, pos: usize) -> usize {
        pos + 1
    }
}

#[test]
fn a_parenthesized_closure_learns_its_parameters_from_later_arguments() {
    let input = Input;
    assert_eq!(call_custom((|input, pos| input.ident(pos)), &input, 4), 5);
    assert_eq!(call_custom(((|input, pos| input.ident(pos))), &input, 1), 2);
    assert_eq!(call_custom(|input, pos| input.ident(pos), &input, 7), 8);
}
