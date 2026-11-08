//~ E0308
//~ help: expected a tuple with 3 elements, found one with 2 elements
fn main() {
    let (a, b) = (1u8, 2u8, 3u8);
    println!("{}", a + b);
}
