// M10: derive(Clone) reads the extension of a pre-edit value.
// The clone gets an independent extension: changing its field keeps the original.

use std::sync::Mutex;

#[derive(Clone)]
struct Item {
    name: String,
    n: u32,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());

#[test]
fn run() {
    let mut items = ITEMS.lock().unwrap();
    if items.is_empty() {
        items.push(Item { name: "old".into(), n: 10 });
    }
    println!("out original {} {}", items[0].name, items[0].n);
    items[0].n += 1;
}
