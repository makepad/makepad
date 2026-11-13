// A discriminant that needs its own enum's size: the enum's layout needs that discriminant, a
// const-eval cycle (rustc: "E0391: cycle detected when simplifying constant for the type system
// `E::A::{constant#0}`"). Relax records the cycle while the program runs; its error line must
// reach stderr, with `--quiet` as well, and `E::A as u8` has no value: its use panics with the
// constant's error instead of printing what was computed across the cycle.
#[repr(u8)]
#[derive(Clone, Copy)]
enum E {
    A = std::mem::size_of::<E>() as u8,
}

fn main() {
    println!("{}", E::A as u8);
}
