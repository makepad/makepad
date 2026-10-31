// G9 / 7.1: adding items, the commonest edit.
//  v1: a new helper fn, called from kernel.
//  v2: a new struct with a Display impl and a derive, used from kernel.

fn helper(x: u64) -> u64 {
    x * 100
}

#[derive(Clone, Debug)]
struct Tag(u64);

impl std::fmt::Display for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "<{}>", self.0)
    }
}

fn kernel(x: u64) -> String {
    let t = Tag(helper(x) + 1);
    format!("{} {:?}", t, t.clone())
}

#[test]
fn run() {
    println!("out {}", kernel(41));
}
