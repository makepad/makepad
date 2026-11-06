// A trait with a provided method goes with its impl. The removed impl's fns are taken out of its
// list at the commit, and the impl was then taken for a marker impl: every instance over its type
// was rebuilt, the trait's `kb` at `A` among them ("failed to resolve instance for Words::bytes").
pub trait Words {
    fn bytes(&self) -> u32;
    fn kb(&self) -> u32 {
        self.bytes() / 1024 + 1
    }
}
pub struct A(u32);
impl Words for A {
    fn bytes(&self) -> u32 {
        self.0 * 4
    }
}

fn report(a: &A) -> u32 {
    a.kb() + a.bytes()
}

#[test]
fn run() {
    println!("out {}", report(&A(600)));
}
