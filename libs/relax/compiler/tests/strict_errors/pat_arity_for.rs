//~ E0308
//~ help: expected a tuple with 3 elements, found one with 2 elements
fn main() {
    let mut s = 0u32;
    for (a, b) in vec![(1u32, 2u32, 3u32)] {
        s += a + b;
    }
    println!("{s}");
}
