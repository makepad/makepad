//~ E0308
//~ help: expected a tuple with 3 elements, found one with 2 elements
// a closure's tuple pattern against references to 3-tuples (a by-reference binding mode)
fn main() {
    let made = [(1u32, 2u8, 3u16)];
    let firsts: Vec<u32> = made.iter().map(|(a, _b)| *a).collect();
    println!("{firsts:?}");
}
