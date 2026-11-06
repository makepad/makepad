// An override is added, changed and removed while generic code, another provided method and a fn
// pointer reach the method without naming the impl: all run the new one.
pub trait Named {
    fn name(&self) -> u32 {
        1
    }
    fn twice(&self) -> u32 {
        self.name() * 2
    }
}
pub struct A;
pub struct B;
impl Named for A {}
impl Named for B {}
fn show<T: Named>(t: &T) -> u32 {
    t.name() + 100
}

#[test]
fn run() {
    let f: fn(&A) -> u32 = <A as Named>::name;
    println!("out {} {} {} {} {}", A.name(), show(&A), A.twice(), f(&A), show(&B));
}
