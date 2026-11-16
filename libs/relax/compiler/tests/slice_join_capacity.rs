// `[V].join(sep)` of vectors allocates once, the joined length, as alloc's `Join` for `[V]` does,
// so its capacity is its length. Relax grew the vector from empty and allocated more often and more
// (relax issue 231: a counting allocator's numbers for joins). String joins and concats were exact.
// Differential file: `| ` lines must match under rustc --test and makepad test.

#[test]
fn a01_joins_and_concats_allocate_their_length() {
    let vs = [vec![1u8, 2], vec![3], vec![], vec![4, 5, 6]];
    let by_item = vs.join(&0u8);
    let by_slice = vs.join(&[9u8, 9][..]);
    let concat = vs.concat();
    let none: Vec<u8> = Vec::<Vec<u8>>::new().join(&0u8);
    let words = ["ab", "c", "", "def"].join(", ");
    let strings = [String::from("x"), String::from("yz")].concat();
    println!("| {:?} cap {}", by_item, by_item.capacity());
    println!("| {:?} cap {}", by_slice, by_slice.capacity());
    println!("| {:?} cap {}", concat, concat.capacity());
    println!("| {:?} cap {}", none, none.capacity());
    println!("| {:?} cap {}", words, words.capacity());
    println!("| {:?} cap {}", strings, strings.capacity());
}
