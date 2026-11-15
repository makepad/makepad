// The crates Relax provides (std/substitutes.txt) keep the features the graph enables: smallvec's
// `write` and arrayvec's default `std` add `io::Write` (relax issue 117: the provided crates were
// loaded without features, and `write_all` was "no method named `write_all`").
use std::io::Write;

fn main() {
    let mut v: smallvec::SmallVec<[u8; 8]> = smallvec::SmallVec::new();
    v.write_all(b"hi").unwrap();
    let mut a = arrayvec::ArrayVec::<u8, 4>::new();
    let n = a.write(b"hello").unwrap();
    println!("{:?} {} {:?}", v, n, a);
}
