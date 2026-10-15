use attr_mac::{add_const, double_body};

#[add_const(SEVEN = 7)]
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct P(u32);

// on an impl block (the shape generated `#[shader] impl X {}` / `#[handle_forward] impl X {}` have)
#[add_const(EIGHT = 8)]
impl P {
    fn get(&self) -> u32 {
        self.0
    }
}

#[double_body]
fn twenty_one() -> u32 {
    21
}

mod inner {
    #[attr_mac::double_body]
    pub fn five() -> u32 { 5 }
}

fn main() {
    assert_eq!(SEVEN, 7);
    assert_eq!(P(1), P(1).clone());
    assert_eq!(twenty_one(), 42);
    assert_eq!(inner::five(), 10);
    assert_eq!(P(EIGHT).get(), 8);
    println!("attr macros ok {} {} {} {}", SEVEN, twenty_one(), inner::five(), P(EIGHT).get());
}
