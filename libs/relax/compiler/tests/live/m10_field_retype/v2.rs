use std::sync::Mutex;

struct Item {
    name: String,
    n: String,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());

#[test]
fn run() {
    let mut items = ITEMS.lock().unwrap();
    if items.is_empty() {
        items.push(Item { name: "kept".into(), n: "fresh".into() });
    }
    for item in items.iter_mut() {
        println!("out {} {:?}", item.name, item.n);
        item.n.push('x');
    }
}
