use std::sync::Mutex;

struct Item {
    name: String,
    n: u32,
    done: bool,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());

#[test]
fn run() {
    let mut items = ITEMS.lock().unwrap();
    if items.is_empty() {
        items.push(Item { name: "a".into(), n: 10, done: false });
        items.push(Item { name: "b".into(), n: 20, done: false });
        items.push(Item { name: "c".into(), n: 30, done: false });
    }
    for item in items.iter() {
        println!("out {} {} {}", item.name, item.n, item.done);
    }
    for item in items.iter_mut() {
        item.n += 1;
        item.done = true;
    }
}
