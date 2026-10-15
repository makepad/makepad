use std::sync::Mutex;

struct Item {
    name: String,
    count: u32,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());

#[test]
fn run() {
    let mut items = ITEMS.lock().unwrap();
    if items.is_empty() {
        items.push(Item { name: "kept".into(), count: 40 });
    }
    for item in items.iter_mut() {
        println!("out {} {}", item.name, item.count);
        item.count += 1;
    }
}
