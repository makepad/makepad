// A fn imported by `use` gets a new signature: its import binds the new fn (it bound the old one,
// and the call was refused: "this function takes 1 argument but 2 were supplied").
mod w {
    pub fn f(x: u32) -> u32 {
        x + 1
    }
}
use w::f;

#[test]
fn run() {
    println!("out {}", f(2));
}
