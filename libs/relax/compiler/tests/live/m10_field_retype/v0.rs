// M10: u32 -> u64 preserves the live value by lossless widening.
// u64 -> String cannot convert and defaults to "", not the new literal.
// No diagnostic wording is required; exact state checks the reset.

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
        items.push(Item { name: "kept".into(), n: 4_000_000_000 });
    }
    for item in items.iter_mut() {
        println!("out {} {}", item.name, item.n);
        item.n += 1;
    }
}
