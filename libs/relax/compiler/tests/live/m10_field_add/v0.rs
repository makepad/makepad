// M10: three non-Copy items survive in a static Vec. v1 adds bool (false);
// v2 grows the extension block with u64 (Default = 0), preserving done.
// v3 gives only a newly created item score 7. v0 runs twice before v1.

use std::sync::Mutex;

struct Item {
    name: String,
    n: u32,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());

#[test]
fn run() {
    let mut items = ITEMS.lock().unwrap();
    if items.is_empty() {
        items.push(Item { name: "a".into(), n: 10 });
        items.push(Item { name: "b".into(), n: 20 });
        items.push(Item { name: "c".into(), n: 30 });
    }
    for item in items.iter() {
        println!("out {} {}", item.name, item.n);
    }
    for item in items.iter_mut() {
        item.n += 1;
    }
}
