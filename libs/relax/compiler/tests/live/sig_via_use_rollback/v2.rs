// As sig_via_use, and the patch panics: the rollback binds the import to the old fn again, and v2
// calls it.
mod w {
    pub fn f(x: u32) -> u32 {
        x + 1
    }
}
use w::f;

#[test]
fn run() {
    println!("out {} again", f(5));
}
