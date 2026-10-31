use std::sync::Mutex;

struct Item {
    name: String,
    n: u32,
    done: bool,
    score: u64,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());

#[test]
fn run() {
    let mut items = ITEMS.lock().unwrap();
    if items.is_empty() {
        items.push(Item { name: "a".into(), n: 10, done: false, score: Default::default() });
        items.push(Item { name: "b".into(), n: 20, done: false, score: Default::default() });
        items.push(Item { name: "c".into(), n: 30, done: false, score: Default::default() });
    }
    for item in items.iter() {
        println!("out {} {} {} {}", item.name, item.n, item.done, item.score);
    }
    for item in items.iter_mut() {
        item.n += 1;
        item.done = true;
    }
}
