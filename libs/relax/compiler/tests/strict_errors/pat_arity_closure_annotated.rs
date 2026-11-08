//~ E0308
//~ help: expected a tuple with 3 elements, found one with 2 elements
fn main() {
    let f = |(a, b): (u8, u8, u8)| a + b;
    println!("{}", f((1, 2, 3)));
}
