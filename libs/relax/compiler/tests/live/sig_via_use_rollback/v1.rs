// As sig_via_use, and the patch panics: the rollback binds the import to the old fn again, and v2
// calls it.
mod w {
    pub fn f(x: u64, y: u64) -> u64 {
        x * 100 + y
    }
}
use w::f;

#[test]
fn run() {
    let v = f(2, 3);
    if v > 0 {
        panic!("rolled back");
    }
    println!("out {}", v);
}
