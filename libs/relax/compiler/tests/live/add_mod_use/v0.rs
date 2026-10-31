// add_mod_use (from add_items v2): a new inline module with a fn, plus a `use` of it.
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
