// A fn goes and its `use` stays: refused (E0432), each time. The import went on binding the removed
// fn, and v2 called it: the removed code ran.
mod w {
    pub fn f() -> u32 {
        7
    }
}
use w::f;

#[test]
fn run() {
    println!("out {}", f());
}
