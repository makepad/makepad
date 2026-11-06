// A trait of a module goes and its `use` stays: refused (E0432). The import went on binding the
// removed trait.
mod w {
    pub struct A(pub u32);
}
use w::Words;

fn size(a: &w::A) -> u32 {
    a.0 * 40
}

#[test]
fn run() {
    println!("out {}", size(&w::A(5)));
}
