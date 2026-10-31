// The true positive next to GitHub #1327: with `K = (String, W)` the one `Borrow` impl makes
// `Q = (String, W)`, and `&s` is no `String`: rustc's E0308 is at the element.
use std::collections::HashMap;
fn main() {
    let mut m: HashMap<(String, u8), u8> = HashMap::new();
    m.insert(("x".to_string(), 1), 2);
    let s = String::from("x");
    println!("{:?}", m.get(&(&s, 1)));
}
