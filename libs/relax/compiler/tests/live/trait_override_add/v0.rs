// A trait impl gets an override of the trait's provided method, loses it and gets another: each
// edit patches. The provided method's instance at the type runs the override through a forwarder,
// and back (it restarted).
pub trait Named {
    fn name(&self) -> u32 {
        1
    }
}
pub struct A;
impl Named for A {}

#[test]
fn run() {
    println!("out {}", A.name());
}
