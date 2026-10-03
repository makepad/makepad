// A foreign fn the host lacks (another platform's API) compiles; calling it panics with a
// one-line report naming the symbol, and the next test runs. Intentionally fails one test.
extern "C" {
    fn hotrust_no_such_symbol_xyz(a: i32) -> i32;
    fn abs(x: i32) -> i32;
}
fn maybe(call: bool) -> i32 {
    if call { unsafe { hotrust_no_such_symbol_xyz(3) } } else { unsafe { abs(-4) } }
}
#[test]
fn not_called() { println!("{}", maybe(false)); }
#[test]
fn called() { println!("{}", maybe(true)); }
#[test]
fn after() { println!("after"); }
