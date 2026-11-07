// body_type_alias: a type alias in a fn body is part of that body. v1 widens it (a pair to a
// triple) and the body uses the third field: the body's edit, checked against the new type.
fn kernel(x: u64) -> u64 {
    type Found = (u64, u64, u64);
    let found: Vec<Found> = (0..x).map(|i| (i, i * i, 7)).collect();
    found.iter().map(|f| f.0 + f.1 + f.2).sum()
}

#[test]
fn run() {
    println!("out {}", kernel(10));
}
