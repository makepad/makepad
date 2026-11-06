// A trait of a module goes with its `use`: patched live.
mod w {
    pub struct A(pub u32);
}

fn size(a: &w::A) -> u32 {
    a.0 * 40
}

#[test]
fn run() {
    println!("out {}", size(&w::A(5)));
}
