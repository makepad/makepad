//~ E0308
//~ help: expected a tuple with 3 elements, found one with 2 elements
fn sum((a, b): (u8, u8, u8)) -> u8 {
    a + b
}
fn main() {
    println!("{}", sum((1, 2, 3)));
}
