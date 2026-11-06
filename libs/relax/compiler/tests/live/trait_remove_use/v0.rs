// A trait of a module goes with its `use`: patched live.
mod w {
    pub trait Words {
        fn bytes(&self) -> u32;
    }
    pub struct A(pub u32);
    impl Words for A {
        fn bytes(&self) -> u32 {
            self.0 * 4
        }
    }
}
use w::Words;

fn size(a: &w::A) -> u32 {
    a.bytes()
}

#[test]
fn run() {
    println!("out {}", size(&w::A(5)));
}
