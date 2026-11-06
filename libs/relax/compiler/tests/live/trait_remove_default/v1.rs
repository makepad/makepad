// A trait with a provided method goes with its impl. The removed impl's fns are taken out of its
// list at the commit, and the impl was then taken for a marker impl: every instance over its type
// was rebuilt, the trait's `kb` at `A` among them ("failed to resolve instance for Words::bytes").
pub struct A(u32);

fn report(a: &A) -> u32 {
    a.0 * 4 / 1024 + 1 + a.0 * 4 + 1000
}

#[test]
fn run() {
    println!("out {}", report(&A(600)));
}
