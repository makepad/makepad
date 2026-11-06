// A struct goes and a field of another (derived) struct still names it: refused (E0412 at the
// field, once).
#[derive(Clone, Copy)]
pub struct Held {
    s: Option<&'static S>,
    n: u32,
}

#[test]
fn run() {
    let h = Held { s: None, n: 3 };
    println!("out {}", h.n);
}
