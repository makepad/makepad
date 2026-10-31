// A panic raised by a destructor while a panic unwinds through interpreted frames aborts the
// process ("panic in a destructor during cleanup"), as rustc does. Run by
// bench_rt/interp/unwind_cmp.py, which expects the process to abort under rustc, the JIT and the
// interpreter alike.
struct Bomb;
impl Drop for Bomb {
    fn drop(&mut self) {
        panic!("second panic in drop");
    }
}
#[inline(never)]
fn xi_inner() {
    let _b = Bomb;
    panic!("first panic");
}
#[inline(never)]
fn xj_outer() {
    xi_inner();
}
#[test]
fn x01_double_panic_aborts() {
    std::panic::set_hook(Box::new(|_| {}));
    let _ = std::panic::catch_unwind(|| xj_outer());
    println!("| not reached");
}
