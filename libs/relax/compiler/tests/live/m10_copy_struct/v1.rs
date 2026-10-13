use std::sync::Mutex;

#[derive(Clone, Copy)]
struct Item {
    n: u32,
    done: bool,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());

#[test]
fn run() {
    let mut items = ITEMS.lock().unwrap();
    if items.is_empty() {
        items.push(Item { n: 10, done: false });
    }
    for item in items.iter_mut() {
        println!("out n {}", item.n);
        println!("out done {}", item.done);
        item.n = 11;
    }
}
