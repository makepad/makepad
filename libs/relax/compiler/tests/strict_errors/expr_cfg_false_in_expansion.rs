//~ msg: removing an expression is not supported in this position
// A macro_rules expansion in expression position (a match arm) whose expression carries a false
// `#[cfg]`: only a statement or a list element is configured away, so rustc reports this at the
// attribute (after E0658 for the attribute itself) instead of removing the arm's value. The same
// macro in a block's tail is a statement and is removed (tests/macro_tail_cfg.rs).
macro_rules! trace { ($($arg:tt)*) => { #[cfg(any())] { undefined::trace!($($arg)+); } } }
fn f(x: u8) {
    match x {
        0 => trace!("zero"),
        _ => { trace!("other") }
    }
}
fn main() { f(0); f(1); }
