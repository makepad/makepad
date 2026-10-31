// `thread_local!` with no space where the type meets the initializer: the lexer joins `>` and `=`
// into `>=` (and `>>` and `=` into `>>=`); rustc's macro matcher splits them, so both are valid.
use std::cell::RefCell;
thread_local! {
    static ONE:RefCell<u32>=RefCell::new(1);
    static TWO:RefCell<Vec<u32>>=const{RefCell::new(Vec::new())};
}

#[test]
fn joint_ge_and_shr_eq_split_at_the_initializer() {
    ONE.with(|v| assert_eq!(*v.borrow(), 1));
    TWO.with(|v| { v.borrow_mut().push(2); assert_eq!(*v.borrow(), vec![2]); });
}
