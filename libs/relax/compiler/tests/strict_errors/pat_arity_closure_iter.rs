//~ E0308
//~ help: expected a tuple with 3 elements, found one with 2 elements
// a closure's tuple pattern against the items of an iterator of 3-tuples (its parameter's type
// comes from the adapter's bound)
fn main() {
    let made: Vec<(u32, String, f64)> = vec![(1, String::from("a"), 2.0)];
    let ids: Vec<u32> = made.into_iter().filter_map(|(id, m)| if m.is_empty() { None } else { Some(id) }).collect();
    println!("{ids:?}");
}
