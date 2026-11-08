//~ E0308
// a closure checked before its parameter's type is known: the call that fixes it comes later
fn main() {
    let f = |(a, b)| a + b;
    let t: (u8, u8, u8) = (1, 2, 3);
    println!("{}", f(t));
}
